#![no_std]

extern crate alloc;

#[cfg(any(feature = "immediate-commands", feature = "epoch-commands"))]
use alloc::string::ToString;
use alloc::{string::String, vec::Vec};
#[cfg(any(feature = "immediate-commands", feature = "epoch-commands"))]
use core::{fmt, str::FromStr};
use soroban_sdk::contracttype;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CodecError {
    Truncated,
    InvalidUtf8,
    InvalidTag,
    InvalidEncoding,
}

pub const VAULT_ERR_INVALID_INPUT: u32 = 3;
pub const VAULT_ERR_ALREADY_INITIALIZED: u32 = 8;

/// Runtime includes recovery action handlers.
pub const RUNTIME_FEATURE_ACTION_RECOVERY: u64 = 1 << 0;
/// Runtime includes external-asset synchronization action handlers.
pub const RUNTIME_FEATURE_ACTION_SYNC_EXTERNAL: u64 = 1 << 1;
/// Runtime includes fee-refresh action handlers.
pub const RUNTIME_FEATURE_ACTION_REFRESH_FEES: u64 = 1 << 2;
/// Runtime includes allocation lifecycle action handlers.
pub const RUNTIME_FEATURE_ACTION_ALLOCATION_LIFECYCLE: u64 = 1 << 3;
/// Runtime includes refresh lifecycle action handlers.
pub const RUNTIME_FEATURE_ACTION_REFRESH_LIFECYCLE: u64 = 1 << 4;
/// Runtime includes pause action handlers.
pub const RUNTIME_FEATURE_ACTION_PAUSE: u64 = 1 << 5;
/// Runtime includes epoch settlement action handlers.
pub const RUNTIME_FEATURE_ACTION_EPOCH_SETTLEMENT: u64 = 1 << 7;
/// Runtime can authorize upgrades of vault companion contracts such as adapters.
pub const RUNTIME_FEATURE_COMPANION_UPGRADE: u64 = 1 << 6;

/// Package version returned for deployed runtimes without a version entrypoint.
pub const RUNTIME_V1_VERSION: &str = "1.0.0";

/// Feature mask shipped by the original v1 runtime.
pub const RUNTIME_V1_FEATURE_FLAGS: u64 = RUNTIME_FEATURE_ACTION_RECOVERY
    | RUNTIME_FEATURE_ACTION_SYNC_EXTERNAL
    | RUNTIME_FEATURE_ACTION_REFRESH_FEES
    | RUNTIME_FEATURE_ACTION_ALLOCATION_LIFECYCLE
    | RUNTIME_FEATURE_ACTION_REFRESH_LIFECYCLE
    | RUNTIME_FEATURE_ACTION_PAUSE;

/// Feature mask enabled by the current default runtime build.
pub const RUNTIME_DEFAULT_FEATURE_FLAGS: u64 =
    RUNTIME_V1_FEATURE_FLAGS | RUNTIME_FEATURE_ACTION_EPOCH_SETTLEMENT;

/// Feature mask enabled by the dedicated epoch settlement runtime build.
#[cfg(feature = "epoch-commands")]
pub const RUNTIME_EPOCH_FEATURE_FLAGS: u64 =
    RUNTIME_FEATURE_ACTION_PAUSE | RUNTIME_FEATURE_ACTION_EPOCH_SETTLEMENT;

/// Compact callable runtime version response.
pub type RuntimeVersionResponse = (soroban_sdk::String, u64);

pub mod strkey {
    use super::CodecError;

    const STRKEY_LEN: usize = 56;
    const BINARY_LEN: usize = 35;
    const ACCOUNT_VERSION: u8 = 6 << 3;
    const CONTRACT_VERSION: u8 = 2 << 3;

    pub fn validate_address_strkey(bytes: &[u8]) -> Result<(), CodecError> {
        if bytes.len() != STRKEY_LEN {
            return Err(CodecError::InvalidEncoding);
        }

        let mut out = [0u8; BINARY_LEN];
        let mut buffer = 0u16;
        let mut bits = 0u8;
        let mut cursor = 0usize;
        for byte in bytes {
            let value = match byte {
                b'A'..=b'Z' => byte - b'A',
                b'2'..=b'7' => byte - b'2' + 26,
                _ => return Err(CodecError::InvalidEncoding),
            };
            buffer = (buffer << 5) | u16::from(value);
            bits += 5;
            if bits >= 8 {
                bits -= 8;
                if cursor >= BINARY_LEN {
                    return Err(CodecError::InvalidEncoding);
                }
                out[cursor] = (buffer >> bits) as u8;
                cursor += 1;
                buffer &= (1u16 << bits) - 1;
            }
        }

        if cursor != BINARY_LEN
            || bits != 0
            || (out[0] != ACCOUNT_VERSION && out[0] != CONTRACT_VERSION)
        {
            return Err(CodecError::InvalidEncoding);
        }

        let expected = u16::from_le_bytes([out[BINARY_LEN - 2], out[BINARY_LEN - 1]]);
        let actual = crc16_xmodem(&out[..BINARY_LEN - 2]);
        if expected != actual {
            return Err(CodecError::InvalidEncoding);
        }

        Ok(())
    }

    #[must_use]
    pub fn crc16_xmodem(bytes: &[u8]) -> u16 {
        let mut crc = 0u16;
        for byte in bytes {
            crc ^= u16::from(*byte) << 8;
            for _ in 0..8 {
                if crc & 0x8000 == 0 {
                    crc <<= 1;
                } else {
                    crc = (crc << 1) ^ 0x1021;
                }
            }
        }
        crc
    }
}

#[cfg(feature = "immediate-commands")]
pub type ProxyAddressesView = (
    soroban_sdk::Address,
    soroban_sdk::Address,
    soroban_sdk::Address,
    soroban_sdk::Address,
);
#[cfg(feature = "immediate-commands")]
pub type ProxyVirtualOffsetsView = (i128, i128, bool);
#[cfg(feature = "immediate-commands")]
pub type ProxyTotalsView = (i128, i128, i128, i128);
#[cfg(feature = "immediate-commands")]
pub type ProxyFeesView = (i128, u64, i128, i128, i128);
#[cfg(feature = "immediate-commands")]
pub type ProxyCoreView = (
    ProxyAddressesView,
    ProxyVirtualOffsetsView,
    ProxyTotalsView,
    ProxyFeesView,
);
#[cfg(feature = "immediate-commands")]
pub type ProxyCapGroupView = (soroban_sdk::String, i128, i128);
#[cfg(feature = "immediate-commands")]
pub type ProxyPolicyView = (soroban_sdk::Vec<u32>, soroban_sdk::Vec<ProxyCapGroupView>);
#[cfg(feature = "immediate-commands")]
pub type ProxyPreviewView = (i128, i128, i128, i128, i128, i128, i128, i128);
#[cfg(feature = "immediate-commands")]
pub type ProxyViewResponse = (ProxyCoreView, ProxyPolicyView, ProxyPreviewView);

#[cfg(feature = "immediate-commands")]
#[derive(Clone)]
pub struct ProxyAddressesFields {
    pub curator: soroban_sdk::Address,
    pub governance: soroban_sdk::Address,
    pub asset_token: soroban_sdk::Address,
    pub share_token: soroban_sdk::Address,
}

#[cfg(feature = "immediate-commands")]
#[derive(Clone)]
pub struct ProxyVirtualOffsetsFields {
    pub virtual_shares: i128,
    pub virtual_assets: i128,
    pub paused: bool,
}

#[cfg(feature = "immediate-commands")]
#[derive(Clone)]
pub struct ProxyTotalsFields {
    pub total_shares: i128,
    pub idle_assets: i128,
    pub external_assets: i128,
    pub total_assets: i128,
}

#[cfg(feature = "immediate-commands")]
#[derive(Clone)]
pub struct ProxyFeesFields {
    pub fee_total_assets: i128,
    pub fee_timestamp_ns: u64,
    pub management_fee_wad: i128,
    pub performance_fee_wad: i128,
    pub max_total_assets_growth_rate_wad: i128,
}

#[cfg(feature = "immediate-commands")]
#[derive(Clone)]
pub struct ProxyCoreFields {
    pub addresses: ProxyAddressesFields,
    pub virtual_offsets: ProxyVirtualOffsetsFields,
    pub totals: ProxyTotalsFields,
    pub fees: ProxyFeesFields,
}

#[cfg(feature = "immediate-commands")]
#[derive(Clone)]
pub struct ProxyPolicyFields {
    pub supply_queue: soroban_sdk::Vec<u32>,
    pub cap_groups: soroban_sdk::Vec<ProxyCapGroupView>,
}

#[cfg(feature = "immediate-commands")]
#[derive(Clone)]
pub struct ProxyPreviewFields {
    pub convert_to_shares: i128,
    pub convert_to_assets: i128,
    pub max_deposit: i128,
    pub max_mint: i128,
    pub max_withdraw: i128,
    pub max_redeem: i128,
    pub preview_mint_assets: i128,
    pub preview_withdraw_shares: i128,
}

#[cfg(feature = "immediate-commands")]
#[derive(Clone)]
pub struct ProxyViewFields {
    pub core: ProxyCoreFields,
    pub policy: ProxyPolicyFields,
    pub preview: ProxyPreviewFields,
}

#[cfg(feature = "immediate-commands")]
impl From<ProxyAddressesView> for ProxyAddressesFields {
    fn from(value: ProxyAddressesView) -> Self {
        let (curator, governance, asset_token, share_token) = value;
        Self {
            curator,
            governance,
            asset_token,
            share_token,
        }
    }
}

#[cfg(feature = "immediate-commands")]
impl From<ProxyVirtualOffsetsView> for ProxyVirtualOffsetsFields {
    fn from(value: ProxyVirtualOffsetsView) -> Self {
        let (virtual_shares, virtual_assets, paused) = value;
        Self {
            virtual_shares,
            virtual_assets,
            paused,
        }
    }
}

#[cfg(feature = "immediate-commands")]
impl From<ProxyTotalsView> for ProxyTotalsFields {
    fn from(value: ProxyTotalsView) -> Self {
        let (total_shares, idle_assets, external_assets, total_assets) = value;
        Self {
            total_shares,
            idle_assets,
            external_assets,
            total_assets,
        }
    }
}

#[cfg(feature = "immediate-commands")]
impl From<ProxyFeesView> for ProxyFeesFields {
    fn from(value: ProxyFeesView) -> Self {
        let (
            fee_total_assets,
            fee_timestamp_ns,
            management_fee_wad,
            performance_fee_wad,
            max_total_assets_growth_rate_wad,
        ) = value;
        Self {
            fee_total_assets,
            fee_timestamp_ns,
            management_fee_wad,
            performance_fee_wad,
            max_total_assets_growth_rate_wad,
        }
    }
}

#[cfg(feature = "immediate-commands")]
impl From<ProxyCoreView> for ProxyCoreFields {
    fn from(value: ProxyCoreView) -> Self {
        let (addresses, virtual_offsets, totals, fees) = value;
        Self {
            addresses: addresses.into(),
            virtual_offsets: virtual_offsets.into(),
            totals: totals.into(),
            fees: fees.into(),
        }
    }
}

#[cfg(feature = "immediate-commands")]
impl From<ProxyPolicyView> for ProxyPolicyFields {
    fn from(value: ProxyPolicyView) -> Self {
        let (supply_queue, cap_groups) = value;
        Self {
            supply_queue,
            cap_groups,
        }
    }
}

#[cfg(feature = "immediate-commands")]
impl From<ProxyPreviewView> for ProxyPreviewFields {
    fn from(value: ProxyPreviewView) -> Self {
        let (
            convert_to_shares,
            convert_to_assets,
            max_deposit,
            max_mint,
            max_withdraw,
            max_redeem,
            preview_mint_assets,
            preview_withdraw_shares,
        ) = value;
        Self {
            convert_to_shares,
            convert_to_assets,
            max_deposit,
            max_mint,
            max_withdraw,
            max_redeem,
            preview_mint_assets,
            preview_withdraw_shares,
        }
    }
}

#[cfg(feature = "immediate-commands")]
impl From<ProxyViewResponse> for ProxyViewFields {
    fn from(value: ProxyViewResponse) -> Self {
        let (core, policy, preview) = value;
        Self {
            core: core.into(),
            policy: policy.into(),
            preview: preview.into(),
        }
    }
}

fn push_u8(out: &mut Vec<u8>, value: u8) {
    out.push(value);
}

fn push_u32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn push_u64(out: &mut Vec<u8>, value: u64) {
    out.extend_from_slice(&value.to_le_bytes());
}

#[cfg(any(feature = "immediate-commands", feature = "epoch-commands"))]
fn push_u128(out: &mut Vec<u8>, value: u128) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn push_i128(out: &mut Vec<u8>, value: i128) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn push_string(out: &mut Vec<u8>, value: &str) {
    let bytes = value.as_bytes();
    push_u32(out, bytes.len() as u32);
    out.extend_from_slice(bytes);
}

fn push_option_i128(out: &mut Vec<u8>, value: &Option<i128>) {
    match value {
        Some(value) => {
            push_u8(out, 1);
            push_i128(out, *value);
        }
        None => push_u8(out, 0),
    }
}

fn push_option_u32(out: &mut Vec<u8>, value: &Option<u32>) {
    match value {
        Some(value) => {
            push_u8(out, 1);
            push_u32(out, *value);
        }
        None => push_u8(out, 0),
    }
}

fn push_option_string(out: &mut Vec<u8>, value: &Option<String>) {
    match value {
        Some(value) => {
            push_u8(out, 1);
            push_string(out, value);
        }
        None => push_u8(out, 0),
    }
}

fn push_u32_vec(out: &mut Vec<u8>, values: &[u32]) {
    push_u32(out, values.len() as u32);
    for value in values {
        push_u32(out, *value);
    }
}

fn push_string_vec(out: &mut Vec<u8>, values: &[String]) {
    push_u32(out, values.len() as u32);
    for value in values {
        push_string(out, value);
    }
}

fn push_option_u32_vec(out: &mut Vec<u8>, values: &Option<Vec<u32>>) {
    match values {
        Some(values) => {
            push_u8(out, 1);
            push_u32_vec(out, values);
        }
        None => push_u8(out, 0),
    }
}

fn push_option_string_vec(out: &mut Vec<u8>, values: &Option<Vec<String>>) {
    match values {
        Some(values) => {
            push_u8(out, 1);
            push_string_vec(out, values);
        }
        None => push_u8(out, 0),
    }
}

#[cfg(feature = "epoch-commands")]
fn push_option_u64(out: &mut Vec<u8>, value: &Option<u64>) {
    match value {
        Some(value) => {
            push_u8(out, 1);
            push_u64(out, *value);
        }
        None => push_u8(out, 0),
    }
}

#[cfg(feature = "epoch-commands")]
fn push_bytes32(out: &mut Vec<u8>, value: &[u8; 32]) {
    out.extend_from_slice(value);
}

#[cfg(feature = "epoch-commands")]
fn push_option_bytes32(out: &mut Vec<u8>, value: &Option<[u8; 32]>) {
    match value {
        Some(value) => {
            push_u8(out, 1);
            push_bytes32(out, value);
        }
        None => push_u8(out, 0),
    }
}

fn read_exact<'a>(bytes: &'a [u8], cursor: &mut usize, len: usize) -> Result<&'a [u8], CodecError> {
    let end = cursor.checked_add(len).ok_or(CodecError::Truncated)?;
    let slice = bytes.get(*cursor..end).ok_or(CodecError::Truncated)?;
    *cursor = end;
    Ok(slice)
}

fn read_u8(bytes: &[u8], cursor: &mut usize) -> Result<u8, CodecError> {
    Ok(read_exact(bytes, cursor, 1)?[0])
}

fn read_u32(bytes: &[u8], cursor: &mut usize) -> Result<u32, CodecError> {
    let mut raw = [0u8; 4];
    raw.copy_from_slice(read_exact(bytes, cursor, 4)?);
    Ok(u32::from_le_bytes(raw))
}

fn read_u64(bytes: &[u8], cursor: &mut usize) -> Result<u64, CodecError> {
    let mut raw = [0u8; 8];
    raw.copy_from_slice(read_exact(bytes, cursor, 8)?);
    Ok(u64::from_le_bytes(raw))
}

#[cfg(any(feature = "immediate-commands", feature = "epoch-commands"))]
fn read_u128(bytes: &[u8], cursor: &mut usize) -> Result<u128, CodecError> {
    let mut raw = [0u8; 16];
    raw.copy_from_slice(read_exact(bytes, cursor, 16)?);
    Ok(u128::from_le_bytes(raw))
}

fn read_i128(bytes: &[u8], cursor: &mut usize) -> Result<i128, CodecError> {
    let mut raw = [0u8; 16];
    raw.copy_from_slice(read_exact(bytes, cursor, 16)?);
    Ok(i128::from_le_bytes(raw))
}

fn read_string(bytes: &[u8], cursor: &mut usize) -> Result<String, CodecError> {
    let len = read_u32(bytes, cursor)? as usize;
    let raw = read_exact(bytes, cursor, len)?;
    String::from_utf8(raw.to_vec()).map_err(|_| CodecError::InvalidUtf8)
}

fn read_option_i128(bytes: &[u8], cursor: &mut usize) -> Result<Option<i128>, CodecError> {
    match read_u8(bytes, cursor)? {
        0 => Ok(None),
        1 => Ok(Some(read_i128(bytes, cursor)?)),
        _ => Err(CodecError::InvalidTag),
    }
}

fn read_option_u32(bytes: &[u8], cursor: &mut usize) -> Result<Option<u32>, CodecError> {
    match read_u8(bytes, cursor)? {
        0 => Ok(None),
        1 => Ok(Some(read_u32(bytes, cursor)?)),
        _ => Err(CodecError::InvalidTag),
    }
}

fn read_option_string(bytes: &[u8], cursor: &mut usize) -> Result<Option<String>, CodecError> {
    match read_u8(bytes, cursor)? {
        0 => Ok(None),
        1 => Ok(Some(read_string(bytes, cursor)?)),
        _ => Err(CodecError::InvalidTag),
    }
}

fn read_u32_vec(bytes: &[u8], cursor: &mut usize) -> Result<Vec<u32>, CodecError> {
    let len = read_u32(bytes, cursor)? as usize;
    let mut values = Vec::new();
    for _ in 0..len {
        values.push(read_u32(bytes, cursor)?);
    }
    Ok(values)
}

fn read_string_vec(bytes: &[u8], cursor: &mut usize) -> Result<Vec<String>, CodecError> {
    let len = read_u32(bytes, cursor)? as usize;
    let mut values = Vec::new();
    for _ in 0..len {
        values.push(read_string(bytes, cursor)?);
    }
    Ok(values)
}

#[cfg(feature = "epoch-commands")]
fn read_option_u64(bytes: &[u8], cursor: &mut usize) -> Result<Option<u64>, CodecError> {
    match read_u8(bytes, cursor)? {
        0 => Ok(None),
        1 => Ok(Some(read_u64(bytes, cursor)?)),
        _ => Err(CodecError::InvalidTag),
    }
}

#[cfg(feature = "epoch-commands")]
fn read_bytes32(bytes: &[u8], cursor: &mut usize) -> Result<[u8; 32], CodecError> {
    let mut raw = [0u8; 32];
    raw.copy_from_slice(read_exact(bytes, cursor, 32)?);
    Ok(raw)
}

#[cfg(feature = "epoch-commands")]
fn read_option_bytes32(bytes: &[u8], cursor: &mut usize) -> Result<Option<[u8; 32]>, CodecError> {
    match read_u8(bytes, cursor)? {
        0 => Ok(None),
        1 => Ok(Some(read_bytes32(bytes, cursor)?)),
        _ => Err(CodecError::InvalidTag),
    }
}

fn read_option_u32_vec(bytes: &[u8], cursor: &mut usize) -> Result<Option<Vec<u32>>, CodecError> {
    match read_u8(bytes, cursor)? {
        0 => Ok(None),
        1 => Ok(Some(read_u32_vec(bytes, cursor)?)),
        _ => Err(CodecError::InvalidTag),
    }
}

fn read_option_string_vec(
    bytes: &[u8],
    cursor: &mut usize,
) -> Result<Option<Vec<String>>, CodecError> {
    match read_u8(bytes, cursor)? {
        0 => Ok(None),
        1 => Ok(Some(read_string_vec(bytes, cursor)?)),
        _ => Err(CodecError::InvalidTag),
    }
}

fn ensure_finished(bytes: &[u8], cursor: usize) -> Result<(), CodecError> {
    if cursor == bytes.len() {
        Ok(())
    } else {
        Err(CodecError::InvalidEncoding)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum VaultCommand {
    #[cfg(feature = "immediate-commands")]
    DepositWithMin {
        owner: String,
        receiver: String,
        assets: i128,
        min_shares_out: i128,
    },
    #[cfg(any(feature = "immediate-commands", feature = "epoch-commands"))]
    RequestWithdraw {
        owner: String,
        receiver: String,
        shares: i128,
        min_assets_out: i128,
    },
    #[cfg(any(feature = "immediate-commands", feature = "epoch-commands"))]
    ExecuteWithdraw { caller: String },
    #[cfg(any(feature = "immediate-commands", feature = "epoch-commands"))]
    AbortWithdrawing { caller: String, op_id: u64 },
    #[cfg(feature = "immediate-commands")]
    Allocate {
        caller: String,
        market: u32,
        amount: i128,
        supply: bool,
    },
    #[cfg(feature = "immediate-commands")]
    RefreshMarkets { caller: String, markets: Vec<u32> },
    #[cfg(feature = "immediate-commands")]
    RefreshFees,
    #[cfg(feature = "immediate-commands")]
    AtomicWithdraw {
        owner: String,
        receiver: String,
        operator: String,
        assets: i128,
        max_shares_burned: i128,
    },
    #[cfg(feature = "immediate-commands")]
    AtomicRedeem {
        owner: String,
        receiver: String,
        operator: String,
        shares: i128,
        min_assets_out: i128,
    },
    #[cfg(feature = "immediate-commands")]
    ResyncIdleBalance,
    #[cfg(feature = "immediate-commands")]
    CancelMigration { caller: String },
    #[cfg(feature = "epoch-commands")]
    ConfigureEpochSettlement {
        caller: String,
        max_report_age_ns: u64,
    },
    #[cfg(feature = "epoch-commands")]
    RequestDeposit {
        owner: String,
        assets: i128,
        min_shares_out: i128,
    },
    #[cfg(feature = "epoch-commands")]
    CancelPendingDeposit { owner: String, request_id: u64 },
    #[cfg(feature = "epoch-commands")]
    BeginEpochCutoff { caller: String, cutoff_ns: u64 },
    #[cfg(feature = "epoch-commands")]
    SettleEpoch { caller: String },
    #[cfg(feature = "epoch-commands")]
    AdmitPendingDeposit { caller: String, request_id: u64 },
    #[cfg(feature = "epoch-commands")]
    CancelPendingWithdrawal { owner: String, request_id: u64 },
    #[cfg(feature = "epoch-commands")]
    GetEpochState,
    #[cfg(feature = "epoch-commands")]
    GetEpochSnapshot { epoch_id: u64 },
    #[cfg(feature = "epoch-commands")]
    GetCustodialReportMetadata { market_id: u32 },
    /// Governance-authorized one-time backed seed for a fresh epoch
    /// deployment. The runtime custodies the underlying assets from the
    /// governance caller, verifies the observed balance delta, and only then
    /// applies the kernel one-for-one mint law.
    #[cfg(feature = "epoch-commands")]
    SeedEpochSupply {
        caller: String,
        receiver: String,
        assets: i128,
    },
    #[cfg(feature = "immediate-commands")]
    ExtendTtl,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum GovernanceCommand {
    SetGovernanceConfig {
        kind: u32,
        primary: Option<String>,
        many: Option<Vec<String>>,
        value_a: Option<i128>,
        value_b: Option<i128>,
    },
    SetGovernancePolicy {
        kind: u32,
        target_ids: Option<Vec<u32>>,
        mode: Option<u32>,
        accounts: Option<Vec<String>>,
        market_id: Option<u32>,
        cap_group_id: Option<String>,
        value: Option<i128>,
        value_b: Option<i128>,
        value_c: Option<i128>,
    },
    Skim {
        token: String,
    },
}

#[cfg(feature = "immediate-commands")]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DepositReceipt {
    pub shares_out: i128,
}

#[cfg(any(feature = "immediate-commands", feature = "epoch-commands"))]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RequestWithdrawReceipt {
    pub request_id: u64,
    pub shares_escrowed: i128,
}

#[cfg(any(feature = "immediate-commands", feature = "epoch-commands"))]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReceiptAddress(String);

#[cfg(any(feature = "immediate-commands", feature = "epoch-commands"))]
impl ReceiptAddress {
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    #[must_use]
    pub fn into_string(self) -> String {
        self.0
    }
}

#[cfg(any(feature = "immediate-commands", feature = "epoch-commands"))]
impl TryFrom<String> for ReceiptAddress {
    type Error = CodecError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        strkey::validate_address_strkey(value.as_bytes())?;
        Ok(Self(value))
    }
}

#[cfg(any(feature = "immediate-commands", feature = "epoch-commands"))]
impl FromStr for ReceiptAddress {
    type Err = CodecError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::try_from(value.to_string())
    }
}

#[cfg(any(feature = "immediate-commands", feature = "epoch-commands"))]
impl fmt::Display for ReceiptAddress {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

#[cfg(any(feature = "immediate-commands", feature = "epoch-commands"))]
impl serde::Serialize for ReceiptAddress {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.0)
    }
}

#[cfg(any(feature = "immediate-commands", feature = "epoch-commands"))]
impl<'de> serde::Deserialize<'de> for ReceiptAddress {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = <String as serde::Deserialize>::deserialize(deserializer)?;
        Self::try_from(value).map_err(|_| serde::de::Error::custom("invalid receipt address"))
    }
}

#[cfg(any(feature = "immediate-commands", feature = "epoch-commands"))]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ExecuteWithdrawStatus {
    pub op_state_before: u32,
    pub op_state_after: u32,
    pub assets_transferred: u128,
    pub events_emitted: u32,
}

#[cfg(any(feature = "immediate-commands", feature = "epoch-commands"))]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ExecuteWithdrawReceipt {
    NoPayout {
        status: ExecuteWithdrawStatus,
    },
    Completed {
        request_id: u64,
        owner: ReceiptAddress,
        receiver: ReceiptAddress,
        assets_out: u128,
        shares_burned: u128,
        status: ExecuteWithdrawStatus,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct I128Receipt {
    pub value: i128,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EmptyReceipt;

pub const GOVERNANCE_CONFIG_KIND_CURATOR: u32 = 0;
pub const GOVERNANCE_CONFIG_KIND_GOVERNANCE: u32 = 1;
pub const GOVERNANCE_CONFIG_KIND_SENTINEL: u32 = 2;
pub const GOVERNANCE_CONFIG_KIND_ALLOCATORS: u32 = 4;
pub const GOVERNANCE_CONFIG_KIND_ALLOWED_ADAPTERS: u32 = 5;
pub const GOVERNANCE_CONFIG_KIND_SKIM_RECIPIENT: u32 = 6;
pub const GOVERNANCE_CONFIG_KIND_VIRTUAL_OFFSETS: u32 = 7;
pub const GOVERNANCE_CONFIG_KIND_WITHDRAWAL_COOLDOWN: u32 = 8;
pub const GOVERNANCE_CONFIG_KIND_IDLE_RESYNC_COOLDOWN: u32 = 9;

pub const GOVERNANCE_POLICY_KIND_SUPPLY_QUEUE: u32 = 0;
pub const GOVERNANCE_POLICY_KIND_CAP: u32 = 1;
pub const GOVERNANCE_POLICY_KIND_REMOVE_MARKET: u32 = 2;
pub const GOVERNANCE_POLICY_KIND_RESTRICTIONS: u32 = 3;
pub const GOVERNANCE_POLICY_KIND_GROUP: u32 = 4;
pub const GOVERNANCE_POLICY_KIND_PAUSED: u32 = 5;
pub const GOVERNANCE_POLICY_KIND_FEES: u32 = 6;

const GOVERNANCE_COMMAND_TAG_BASE: u8 = 0x80;
const GOVERNANCE_COMMAND_TAG_SET_CONFIG: u8 = GOVERNANCE_COMMAND_TAG_BASE;
const GOVERNANCE_COMMAND_TAG_SET_POLICY: u8 = GOVERNANCE_COMMAND_TAG_BASE + 1;
const GOVERNANCE_COMMAND_TAG_SKIM: u8 = GOVERNANCE_COMMAND_TAG_BASE + 2;

impl VaultCommand {
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        match self {
            #[cfg(feature = "immediate-commands")]
            Self::DepositWithMin {
                owner,
                receiver,
                assets,
                min_shares_out,
            } => {
                push_u8(&mut out, 0);
                push_string(&mut out, owner);
                push_string(&mut out, receiver);
                push_i128(&mut out, *assets);
                push_i128(&mut out, *min_shares_out);
            }
            #[cfg(any(feature = "immediate-commands", feature = "epoch-commands"))]
            Self::RequestWithdraw {
                owner,
                receiver,
                shares,
                min_assets_out,
            } => {
                push_u8(&mut out, 1);
                push_string(&mut out, owner);
                push_string(&mut out, receiver);
                push_i128(&mut out, *shares);
                push_i128(&mut out, *min_assets_out);
            }
            #[cfg(any(feature = "immediate-commands", feature = "epoch-commands"))]
            Self::ExecuteWithdraw { caller } => {
                push_u8(&mut out, 2);
                push_string(&mut out, caller);
            }
            #[cfg(any(feature = "immediate-commands", feature = "epoch-commands"))]
            Self::AbortWithdrawing { caller, op_id } => {
                push_u8(&mut out, 11);
                push_string(&mut out, caller);
                push_u64(&mut out, *op_id);
            }
            #[cfg(feature = "immediate-commands")]
            Self::Allocate {
                caller,
                market,
                amount,
                supply,
            } => {
                push_u8(&mut out, 3);
                push_string(&mut out, caller);
                push_u32(&mut out, *market);
                push_i128(&mut out, *amount);
                push_u8(&mut out, u8::from(*supply));
            }
            #[cfg(feature = "immediate-commands")]
            Self::RefreshMarkets { caller, markets } => {
                push_u8(&mut out, 4);
                push_string(&mut out, caller);
                push_u32_vec(&mut out, markets);
            }
            #[cfg(feature = "immediate-commands")]
            Self::RefreshFees => push_u8(&mut out, 5),
            #[cfg(feature = "immediate-commands")]
            Self::AtomicWithdraw {
                owner,
                receiver,
                operator,
                assets,
                max_shares_burned,
            } => {
                push_u8(&mut out, 6);
                push_string(&mut out, owner);
                push_string(&mut out, receiver);
                push_string(&mut out, operator);
                push_i128(&mut out, *assets);
                push_i128(&mut out, *max_shares_burned);
            }
            #[cfg(feature = "immediate-commands")]
            Self::AtomicRedeem {
                owner,
                receiver,
                operator,
                shares,
                min_assets_out,
            } => {
                push_u8(&mut out, 7);
                push_string(&mut out, owner);
                push_string(&mut out, receiver);
                push_string(&mut out, operator);
                push_i128(&mut out, *shares);
                push_i128(&mut out, *min_assets_out);
            }
            #[cfg(feature = "immediate-commands")]
            Self::ResyncIdleBalance => push_u8(&mut out, 8),
            #[cfg(feature = "immediate-commands")]
            Self::CancelMigration { caller } => {
                push_u8(&mut out, 9);
                push_string(&mut out, caller);
            }
            #[cfg(feature = "epoch-commands")]
            Self::ConfigureEpochSettlement {
                caller,
                max_report_age_ns,
            } => {
                push_u8(&mut out, 12);
                push_string(&mut out, caller);
                push_u64(&mut out, *max_report_age_ns);
            }
            #[cfg(feature = "epoch-commands")]
            Self::RequestDeposit {
                owner,
                assets,
                min_shares_out,
            } => {
                push_u8(&mut out, 13);
                push_string(&mut out, owner);
                push_i128(&mut out, *assets);
                push_i128(&mut out, *min_shares_out);
            }
            #[cfg(feature = "epoch-commands")]
            Self::CancelPendingDeposit { owner, request_id } => {
                push_u8(&mut out, 14);
                push_string(&mut out, owner);
                push_u64(&mut out, *request_id);
            }
            #[cfg(feature = "epoch-commands")]
            Self::BeginEpochCutoff { caller, cutoff_ns } => {
                push_u8(&mut out, 15);
                push_string(&mut out, caller);
                push_u64(&mut out, *cutoff_ns);
            }
            #[cfg(feature = "epoch-commands")]
            Self::SettleEpoch { caller } => {
                push_u8(&mut out, 16);
                push_string(&mut out, caller);
            }
            #[cfg(feature = "epoch-commands")]
            Self::AdmitPendingDeposit { caller, request_id } => {
                push_u8(&mut out, 17);
                push_string(&mut out, caller);
                push_u64(&mut out, *request_id);
            }
            #[cfg(feature = "epoch-commands")]
            Self::CancelPendingWithdrawal { owner, request_id } => {
                push_u8(&mut out, 18);
                push_string(&mut out, owner);
                push_u64(&mut out, *request_id);
            }
            #[cfg(feature = "epoch-commands")]
            Self::GetEpochState => push_u8(&mut out, 19),
            #[cfg(feature = "epoch-commands")]
            Self::GetEpochSnapshot { epoch_id } => {
                push_u8(&mut out, 20);
                push_u64(&mut out, *epoch_id);
            }
            #[cfg(feature = "epoch-commands")]
            Self::GetCustodialReportMetadata { market_id } => {
                push_u8(&mut out, 21);
                push_u32(&mut out, *market_id);
            }
            #[cfg(feature = "epoch-commands")]
            Self::SeedEpochSupply {
                caller,
                receiver,
                assets,
            } => {
                push_u8(&mut out, 22);
                push_string(&mut out, caller);
                push_string(&mut out, receiver);
                push_i128(&mut out, *assets);
            }
            #[cfg(feature = "immediate-commands")]
            Self::ExtendTtl => push_u8(&mut out, 10),
            #[cfg(not(any(feature = "immediate-commands", feature = "epoch-commands")))]
            _ => {}
        }
        out
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, CodecError> {
        let mut cursor = 0usize;
        let command = match read_u8(bytes, &mut cursor)? {
            #[cfg(feature = "immediate-commands")]
            0 => Ok(Self::DepositWithMin {
                owner: read_string(bytes, &mut cursor)?,
                receiver: read_string(bytes, &mut cursor)?,
                assets: read_i128(bytes, &mut cursor)?,
                min_shares_out: read_i128(bytes, &mut cursor)?,
            }),
            #[cfg(any(feature = "immediate-commands", feature = "epoch-commands"))]
            1 => Ok(Self::RequestWithdraw {
                owner: read_string(bytes, &mut cursor)?,
                receiver: read_string(bytes, &mut cursor)?,
                shares: read_i128(bytes, &mut cursor)?,
                min_assets_out: read_i128(bytes, &mut cursor)?,
            }),
            #[cfg(any(feature = "immediate-commands", feature = "epoch-commands"))]
            2 => Ok(Self::ExecuteWithdraw {
                caller: read_string(bytes, &mut cursor)?,
            }),
            #[cfg(any(feature = "immediate-commands", feature = "epoch-commands"))]
            11 => Ok(Self::AbortWithdrawing {
                caller: read_string(bytes, &mut cursor)?,
                op_id: read_u64(bytes, &mut cursor)?,
            }),
            #[cfg(feature = "immediate-commands")]
            3 => Ok(Self::Allocate {
                caller: read_string(bytes, &mut cursor)?,
                market: read_u32(bytes, &mut cursor)?,
                amount: read_i128(bytes, &mut cursor)?,
                supply: match read_u8(bytes, &mut cursor)? {
                    0 => false,
                    1 => true,
                    _ => return Err(CodecError::InvalidEncoding),
                },
            }),
            #[cfg(feature = "immediate-commands")]
            4 => Ok(Self::RefreshMarkets {
                caller: read_string(bytes, &mut cursor)?,
                markets: read_u32_vec(bytes, &mut cursor)?,
            }),
            #[cfg(feature = "immediate-commands")]
            5 => Ok(Self::RefreshFees),
            #[cfg(feature = "immediate-commands")]
            6 => Ok(Self::AtomicWithdraw {
                owner: read_string(bytes, &mut cursor)?,
                receiver: read_string(bytes, &mut cursor)?,
                operator: read_string(bytes, &mut cursor)?,
                assets: read_i128(bytes, &mut cursor)?,
                max_shares_burned: read_i128(bytes, &mut cursor)?,
            }),
            #[cfg(feature = "immediate-commands")]
            7 => Ok(Self::AtomicRedeem {
                owner: read_string(bytes, &mut cursor)?,
                receiver: read_string(bytes, &mut cursor)?,
                operator: read_string(bytes, &mut cursor)?,
                shares: read_i128(bytes, &mut cursor)?,
                min_assets_out: read_i128(bytes, &mut cursor)?,
            }),
            #[cfg(feature = "immediate-commands")]
            8 => Ok(Self::ResyncIdleBalance),
            #[cfg(feature = "immediate-commands")]
            9 => Ok(Self::CancelMigration {
                caller: read_string(bytes, &mut cursor)?,
            }),
            #[cfg(feature = "immediate-commands")]
            10 => Ok(Self::ExtendTtl),
            #[cfg(feature = "epoch-commands")]
            12 => Ok(Self::ConfigureEpochSettlement {
                caller: read_string(bytes, &mut cursor)?,
                max_report_age_ns: read_u64(bytes, &mut cursor)?,
            }),
            #[cfg(feature = "epoch-commands")]
            13 => Ok(Self::RequestDeposit {
                owner: read_string(bytes, &mut cursor)?,
                assets: read_i128(bytes, &mut cursor)?,
                min_shares_out: read_i128(bytes, &mut cursor)?,
            }),
            #[cfg(feature = "epoch-commands")]
            14 => Ok(Self::CancelPendingDeposit {
                owner: read_string(bytes, &mut cursor)?,
                request_id: read_u64(bytes, &mut cursor)?,
            }),
            #[cfg(feature = "epoch-commands")]
            15 => Ok(Self::BeginEpochCutoff {
                caller: read_string(bytes, &mut cursor)?,
                cutoff_ns: read_u64(bytes, &mut cursor)?,
            }),
            #[cfg(feature = "epoch-commands")]
            16 => Ok(Self::SettleEpoch {
                caller: read_string(bytes, &mut cursor)?,
            }),
            #[cfg(feature = "epoch-commands")]
            17 => Ok(Self::AdmitPendingDeposit {
                caller: read_string(bytes, &mut cursor)?,
                request_id: read_u64(bytes, &mut cursor)?,
            }),
            #[cfg(feature = "epoch-commands")]
            18 => Ok(Self::CancelPendingWithdrawal {
                owner: read_string(bytes, &mut cursor)?,
                request_id: read_u64(bytes, &mut cursor)?,
            }),
            #[cfg(feature = "epoch-commands")]
            19 => Ok(Self::GetEpochState),
            #[cfg(feature = "epoch-commands")]
            20 => Ok(Self::GetEpochSnapshot {
                epoch_id: read_u64(bytes, &mut cursor)?,
            }),
            #[cfg(feature = "epoch-commands")]
            21 => Ok(Self::GetCustodialReportMetadata {
                market_id: read_u32(bytes, &mut cursor)?,
            }),
            #[cfg(feature = "epoch-commands")]
            22 => Ok(Self::SeedEpochSupply {
                caller: read_string(bytes, &mut cursor)?,
                receiver: read_string(bytes, &mut cursor)?,
                assets: read_i128(bytes, &mut cursor)?,
            }),
            _ => Err(CodecError::InvalidTag),
        }?;
        ensure_finished(bytes, cursor)?;
        Ok(command)
    }
}

impl GovernanceCommand {
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        match self {
            Self::SetGovernanceConfig {
                kind,
                primary,
                many,
                value_a,
                value_b,
            } => {
                push_u8(&mut out, GOVERNANCE_COMMAND_TAG_SET_CONFIG);
                push_u32(&mut out, *kind);
                push_option_string(&mut out, primary);
                push_option_string_vec(&mut out, many);
                push_option_i128(&mut out, value_a);
                push_option_i128(&mut out, value_b);
            }
            Self::SetGovernancePolicy {
                kind,
                target_ids,
                mode,
                accounts,
                market_id,
                cap_group_id,
                value,
                value_b,
                value_c,
            } => {
                push_u8(&mut out, GOVERNANCE_COMMAND_TAG_SET_POLICY);
                push_u32(&mut out, *kind);
                push_option_u32_vec(&mut out, target_ids);
                push_option_u32(&mut out, mode);
                push_option_string_vec(&mut out, accounts);
                push_option_u32(&mut out, market_id);
                push_option_string(&mut out, cap_group_id);
                push_option_i128(&mut out, value);
                push_option_i128(&mut out, value_b);
                push_option_i128(&mut out, value_c);
            }
            Self::Skim { token } => {
                push_u8(&mut out, GOVERNANCE_COMMAND_TAG_SKIM);
                push_string(&mut out, token);
            }
        }
        out
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, CodecError> {
        let mut cursor = 0usize;
        let command = match read_u8(bytes, &mut cursor)? {
            GOVERNANCE_COMMAND_TAG_SET_CONFIG => Ok(Self::SetGovernanceConfig {
                kind: read_u32(bytes, &mut cursor)?,
                primary: read_option_string(bytes, &mut cursor)?,
                many: read_option_string_vec(bytes, &mut cursor)?,
                value_a: read_option_i128(bytes, &mut cursor)?,
                value_b: read_option_i128(bytes, &mut cursor)?,
            }),
            GOVERNANCE_COMMAND_TAG_SET_POLICY => Ok(Self::SetGovernancePolicy {
                kind: read_u32(bytes, &mut cursor)?,
                target_ids: read_option_u32_vec(bytes, &mut cursor)?,
                mode: read_option_u32(bytes, &mut cursor)?,
                accounts: read_option_string_vec(bytes, &mut cursor)?,
                market_id: read_option_u32(bytes, &mut cursor)?,
                cap_group_id: read_option_string(bytes, &mut cursor)?,
                value: read_option_i128(bytes, &mut cursor)?,
                value_b: read_option_i128(bytes, &mut cursor)?,
                value_c: read_option_i128(bytes, &mut cursor)?,
            }),
            GOVERNANCE_COMMAND_TAG_SKIM => Ok(Self::Skim {
                token: read_string(bytes, &mut cursor)?,
            }),
            _ => Err(CodecError::InvalidTag),
        }?;
        ensure_finished(bytes, cursor)?;
        Ok(command)
    }
}

#[cfg(feature = "immediate-commands")]
impl DepositReceipt {
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        push_u8(&mut out, 0);
        push_i128(&mut out, self.shares_out);
        out
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, CodecError> {
        let mut cursor = 0usize;
        if read_u8(bytes, &mut cursor)? != 0 {
            return Err(CodecError::InvalidTag);
        }
        let result = Self {
            shares_out: read_i128(bytes, &mut cursor)?,
        };
        ensure_finished(bytes, cursor)?;
        Ok(result)
    }
}

#[cfg(any(feature = "immediate-commands", feature = "epoch-commands"))]
impl RequestWithdrawReceipt {
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        push_u8(&mut out, 1);
        push_u64(&mut out, self.request_id);
        push_i128(&mut out, self.shares_escrowed);
        out
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, CodecError> {
        let mut cursor = 0usize;
        if read_u8(bytes, &mut cursor)? != 1 {
            return Err(CodecError::InvalidTag);
        }
        let result = Self {
            request_id: read_u64(bytes, &mut cursor)?,
            shares_escrowed: read_i128(bytes, &mut cursor)?,
        };
        ensure_finished(bytes, cursor)?;
        Ok(result)
    }
}

#[cfg(any(feature = "immediate-commands", feature = "epoch-commands"))]
impl ExecuteWithdrawReceipt {
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        push_u8(&mut out, 2);
        match self {
            Self::NoPayout { status } => {
                push_u8(&mut out, 0);
                push_u32(&mut out, status.op_state_before);
                push_u32(&mut out, status.op_state_after);
                push_u128(&mut out, status.assets_transferred);
                push_u32(&mut out, status.events_emitted);
            }
            Self::Completed {
                request_id,
                owner,
                receiver,
                assets_out,
                shares_burned,
                status,
            } => {
                push_u8(&mut out, 1);
                push_u64(&mut out, *request_id);
                push_string(&mut out, owner.as_str());
                push_string(&mut out, receiver.as_str());
                push_u128(&mut out, *assets_out);
                push_u128(&mut out, *shares_burned);
                push_u32(&mut out, status.op_state_before);
                push_u32(&mut out, status.op_state_after);
                push_u128(&mut out, status.assets_transferred);
                push_u32(&mut out, status.events_emitted);
            }
        }
        out
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, CodecError> {
        let mut cursor = 0usize;
        if read_u8(bytes, &mut cursor)? != 2 {
            return Err(CodecError::InvalidTag);
        }
        let result = match read_u8(bytes, &mut cursor)? {
            0 => Self::NoPayout {
                status: ExecuteWithdrawStatus {
                    op_state_before: read_u32(bytes, &mut cursor)?,
                    op_state_after: read_u32(bytes, &mut cursor)?,
                    assets_transferred: read_u128(bytes, &mut cursor)?,
                    events_emitted: read_u32(bytes, &mut cursor)?,
                },
            },
            1 => {
                let request_id = read_u64(bytes, &mut cursor)?;
                let owner = ReceiptAddress::try_from(read_string(bytes, &mut cursor)?)?;
                let receiver = ReceiptAddress::try_from(read_string(bytes, &mut cursor)?)?;
                let assets_out = read_u128(bytes, &mut cursor)?;
                let shares_burned = read_u128(bytes, &mut cursor)?;
                let status = ExecuteWithdrawStatus {
                    op_state_before: read_u32(bytes, &mut cursor)?,
                    op_state_after: read_u32(bytes, &mut cursor)?,
                    assets_transferred: read_u128(bytes, &mut cursor)?,
                    events_emitted: read_u32(bytes, &mut cursor)?,
                };
                Self::Completed {
                    request_id,
                    owner,
                    receiver,
                    assets_out,
                    shares_burned,
                    status,
                }
            }
            _ => return Err(CodecError::InvalidTag),
        };
        ensure_finished(bytes, cursor)?;
        Ok(result)
    }
}

impl I128Receipt {
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        push_u8(&mut out, 3);
        push_i128(&mut out, self.value);
        out
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, CodecError> {
        let mut cursor = 0usize;
        if read_u8(bytes, &mut cursor)? != 3 {
            return Err(CodecError::InvalidTag);
        }
        let result = Self {
            value: read_i128(bytes, &mut cursor)?,
        };
        ensure_finished(bytes, cursor)?;
        Ok(result)
    }
}

impl EmptyReceipt {
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        push_u8(&mut out, 4);
        out
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, CodecError> {
        let mut cursor = 0usize;
        if read_u8(bytes, &mut cursor)? != 4 {
            return Err(CodecError::InvalidTag);
        }
        ensure_finished(bytes, cursor)?;
        Ok(Self)
    }
}
/// Epoch lifecycle phase reported by [`EpochStateViewReceipt`]: intake is open.
#[cfg(feature = "epoch-commands")]
pub const EPOCH_PHASE_OPEN: u32 = 0;
#[cfg(feature = "epoch-commands")]
/// Epoch lifecycle phase reported by [`EpochStateViewReceipt`]: intake is
/// closed at a cutoff and settlement is pending.
pub const EPOCH_PHASE_CUTOFF: u32 = 1;
#[cfg(feature = "epoch-commands")]
/// Epoch lifecycle phase reported by [`EpochStateViewReceipt`]: the epoch has
/// settled and an immutable snapshot is bound to it.
pub const EPOCH_PHASE_SETTLED: u32 = 2;

#[cfg(feature = "epoch-commands")]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConfigureEpochSettlementReceipt {
    pub max_report_age_ns: u64,
}

#[cfg(feature = "epoch-commands")]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PendingDepositReceipt {
    pub request_id: u64,
    pub assets: i128,
}

#[cfg(feature = "epoch-commands")]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CancelPendingDepositReceipt {
    pub request_id: u64,
    pub assets_refunded: i128,
}

#[cfg(feature = "epoch-commands")]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BeginEpochCutoffReceipt {
    pub epoch_id: u64,
    pub cutoff_ns: u64,
}

#[cfg(feature = "epoch-commands")]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SettleEpochReceipt {
    pub epoch_id: u64,
    pub report_seq: u64,
    pub as_of_ns: u64,
    pub report_hash: [u8; 32],
    pub settlement_nav: i128,
    pub eligible_supply: i128,
    pub cutoff_ns: u64,
}

#[cfg(feature = "epoch-commands")]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AdmitPendingDepositReceipt {
    pub request_id: u64,
    pub shares_out: i128,
    pub assets_in: i128,
}

#[cfg(feature = "epoch-commands")]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CancelPendingWithdrawalReceipt {
    pub request_id: u64,
    pub shares_refunded: i128,
    pub epoch_id: u64,
}

#[cfg(feature = "epoch-commands")]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EpochStateViewReceipt {
    pub phase: u32,
    pub intake_epoch: u64,
    pub cutoff_ns: Option<u64>,
    pub last_settled_epoch_id: Option<u64>,
    pub last_report_seq: Option<u64>,
}

#[cfg(feature = "epoch-commands")]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EpochSnapshotReceipt {
    pub epoch_id: u64,
    pub report_seq: u64,
    pub as_of_ns: u64,
    pub report_hash: [u8; 32],
    pub settlement_nav: i128,
    pub eligible_supply: i128,
    pub cutoff_ns: u64,
}

#[cfg(feature = "epoch-commands")]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ReportMetadataReceipt {
    Available {
        market_id: u32,
        seq: u64,
        as_of: u64,
        submitted_at: u64,
        assets_value: i128,
        report_hash: Option<[u8; 32]>,
    },
    Unavailable {
        market_id: u32,
    },
}

/// Receipt for a governed one-time backed seed: the exact amount custodied
/// into vault custody and the exactly matching shares minted to the receiver.
#[cfg(feature = "epoch-commands")]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SeedEpochSupplyReceipt {
    pub assets_seeded: i128,
    pub shares_minted: i128,
}
#[cfg(feature = "epoch-commands")]
impl ConfigureEpochSettlementReceipt {
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        push_u8(&mut out, 5);
        push_u64(&mut out, self.max_report_age_ns);
        out
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, CodecError> {
        let mut cursor = 0usize;
        if read_u8(bytes, &mut cursor)? != 5 {
            return Err(CodecError::InvalidTag);
        }
        let result = Self {
            max_report_age_ns: read_u64(bytes, &mut cursor)?,
        };
        ensure_finished(bytes, cursor)?;
        Ok(result)
    }
}

#[cfg(feature = "epoch-commands")]
impl PendingDepositReceipt {
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        push_u8(&mut out, 6);
        push_u64(&mut out, self.request_id);
        push_i128(&mut out, self.assets);
        out
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, CodecError> {
        let mut cursor = 0usize;
        if read_u8(bytes, &mut cursor)? != 6 {
            return Err(CodecError::InvalidTag);
        }
        let result = Self {
            request_id: read_u64(bytes, &mut cursor)?,
            assets: read_i128(bytes, &mut cursor)?,
        };
        ensure_finished(bytes, cursor)?;
        Ok(result)
    }
}

#[cfg(feature = "epoch-commands")]
impl CancelPendingDepositReceipt {
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        push_u8(&mut out, 7);
        push_u64(&mut out, self.request_id);
        push_i128(&mut out, self.assets_refunded);
        out
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, CodecError> {
        let mut cursor = 0usize;
        if read_u8(bytes, &mut cursor)? != 7 {
            return Err(CodecError::InvalidTag);
        }
        let result = Self {
            request_id: read_u64(bytes, &mut cursor)?,
            assets_refunded: read_i128(bytes, &mut cursor)?,
        };
        ensure_finished(bytes, cursor)?;
        Ok(result)
    }
}

#[cfg(feature = "epoch-commands")]
impl BeginEpochCutoffReceipt {
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        push_u8(&mut out, 8);
        push_u64(&mut out, self.epoch_id);
        push_u64(&mut out, self.cutoff_ns);
        out
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, CodecError> {
        let mut cursor = 0usize;
        if read_u8(bytes, &mut cursor)? != 8 {
            return Err(CodecError::InvalidTag);
        }
        let result = Self {
            epoch_id: read_u64(bytes, &mut cursor)?,
            cutoff_ns: read_u64(bytes, &mut cursor)?,
        };
        ensure_finished(bytes, cursor)?;
        Ok(result)
    }
}

#[cfg(feature = "epoch-commands")]
impl SettleEpochReceipt {
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        push_u8(&mut out, 9);
        push_u64(&mut out, self.epoch_id);
        push_u64(&mut out, self.report_seq);
        push_u64(&mut out, self.as_of_ns);
        push_bytes32(&mut out, &self.report_hash);
        push_i128(&mut out, self.settlement_nav);
        push_i128(&mut out, self.eligible_supply);
        push_u64(&mut out, self.cutoff_ns);
        out
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, CodecError> {
        let mut cursor = 0usize;
        if read_u8(bytes, &mut cursor)? != 9 {
            return Err(CodecError::InvalidTag);
        }
        let result = Self {
            epoch_id: read_u64(bytes, &mut cursor)?,
            report_seq: read_u64(bytes, &mut cursor)?,
            as_of_ns: read_u64(bytes, &mut cursor)?,
            report_hash: read_bytes32(bytes, &mut cursor)?,
            settlement_nav: read_i128(bytes, &mut cursor)?,
            eligible_supply: read_i128(bytes, &mut cursor)?,
            cutoff_ns: read_u64(bytes, &mut cursor)?,
        };
        ensure_finished(bytes, cursor)?;
        Ok(result)
    }
}

#[cfg(feature = "epoch-commands")]
impl AdmitPendingDepositReceipt {
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        push_u8(&mut out, 10);
        push_u64(&mut out, self.request_id);
        push_i128(&mut out, self.shares_out);
        push_i128(&mut out, self.assets_in);
        out
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, CodecError> {
        let mut cursor = 0usize;
        if read_u8(bytes, &mut cursor)? != 10 {
            return Err(CodecError::InvalidTag);
        }
        let result = Self {
            request_id: read_u64(bytes, &mut cursor)?,
            shares_out: read_i128(bytes, &mut cursor)?,
            assets_in: read_i128(bytes, &mut cursor)?,
        };
        ensure_finished(bytes, cursor)?;
        Ok(result)
    }
}

#[cfg(feature = "epoch-commands")]
impl CancelPendingWithdrawalReceipt {
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        push_u8(&mut out, 11);
        push_u64(&mut out, self.request_id);
        push_i128(&mut out, self.shares_refunded);
        push_u64(&mut out, self.epoch_id);
        out
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, CodecError> {
        let mut cursor = 0usize;
        if read_u8(bytes, &mut cursor)? != 11 {
            return Err(CodecError::InvalidTag);
        }
        let result = Self {
            request_id: read_u64(bytes, &mut cursor)?,
            shares_refunded: read_i128(bytes, &mut cursor)?,
            epoch_id: read_u64(bytes, &mut cursor)?,
        };
        ensure_finished(bytes, cursor)?;
        Ok(result)
    }
}

#[cfg(feature = "epoch-commands")]
impl EpochStateViewReceipt {
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        push_u8(&mut out, 12);
        push_u32(&mut out, self.phase);
        push_u64(&mut out, self.intake_epoch);
        push_option_u64(&mut out, &self.cutoff_ns);
        push_option_u64(&mut out, &self.last_settled_epoch_id);
        push_option_u64(&mut out, &self.last_report_seq);
        out
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, CodecError> {
        let mut cursor = 0usize;
        if read_u8(bytes, &mut cursor)? != 12 {
            return Err(CodecError::InvalidTag);
        }
        let phase = read_u32(bytes, &mut cursor)?;
        if !matches!(
            phase,
            EPOCH_PHASE_OPEN | EPOCH_PHASE_CUTOFF | EPOCH_PHASE_SETTLED
        ) {
            return Err(CodecError::InvalidEncoding);
        }
        let result = Self {
            phase,
            intake_epoch: read_u64(bytes, &mut cursor)?,
            cutoff_ns: read_option_u64(bytes, &mut cursor)?,
            last_settled_epoch_id: read_option_u64(bytes, &mut cursor)?,
            last_report_seq: read_option_u64(bytes, &mut cursor)?,
        };
        ensure_finished(bytes, cursor)?;
        Ok(result)
    }
}

#[cfg(feature = "epoch-commands")]
impl EpochSnapshotReceipt {
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        push_u8(&mut out, 13);
        push_u64(&mut out, self.epoch_id);
        push_u64(&mut out, self.report_seq);
        push_u64(&mut out, self.as_of_ns);
        push_bytes32(&mut out, &self.report_hash);
        push_i128(&mut out, self.settlement_nav);
        push_i128(&mut out, self.eligible_supply);
        push_u64(&mut out, self.cutoff_ns);
        out
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, CodecError> {
        let mut cursor = 0usize;
        if read_u8(bytes, &mut cursor)? != 13 {
            return Err(CodecError::InvalidTag);
        }
        let result = Self {
            epoch_id: read_u64(bytes, &mut cursor)?,
            report_seq: read_u64(bytes, &mut cursor)?,
            as_of_ns: read_u64(bytes, &mut cursor)?,
            report_hash: read_bytes32(bytes, &mut cursor)?,
            settlement_nav: read_i128(bytes, &mut cursor)?,
            eligible_supply: read_i128(bytes, &mut cursor)?,
            cutoff_ns: read_u64(bytes, &mut cursor)?,
        };
        ensure_finished(bytes, cursor)?;
        Ok(result)
    }
}

#[cfg(feature = "epoch-commands")]
impl ReportMetadataReceipt {
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        push_u8(&mut out, 14);
        match self {
            Self::Available {
                market_id,
                seq,
                as_of,
                submitted_at,
                assets_value,
                report_hash,
            } => {
                push_u8(&mut out, 1);
                push_u32(&mut out, *market_id);
                push_u64(&mut out, *seq);
                push_u64(&mut out, *as_of);
                push_u64(&mut out, *submitted_at);
                push_i128(&mut out, *assets_value);
                push_option_bytes32(&mut out, report_hash);
            }
            Self::Unavailable { market_id } => {
                push_u8(&mut out, 0);
                push_u32(&mut out, *market_id);
            }
        }
        out
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, CodecError> {
        let mut cursor = 0usize;
        if read_u8(bytes, &mut cursor)? != 14 {
            return Err(CodecError::InvalidTag);
        }
        let result = match read_u8(bytes, &mut cursor)? {
            1 => Self::Available {
                market_id: read_u32(bytes, &mut cursor)?,
                seq: read_u64(bytes, &mut cursor)?,
                as_of: read_u64(bytes, &mut cursor)?,
                submitted_at: read_u64(bytes, &mut cursor)?,
                assets_value: read_i128(bytes, &mut cursor)?,
                report_hash: read_option_bytes32(bytes, &mut cursor)?,
            },
            0 => Self::Unavailable {
                market_id: read_u32(bytes, &mut cursor)?,
            },
            _ => return Err(CodecError::InvalidTag),
        };
        ensure_finished(bytes, cursor)?;
        Ok(result)
    }
}

#[cfg(feature = "epoch-commands")]
impl SeedEpochSupplyReceipt {
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        push_u8(&mut out, 15);
        push_i128(&mut out, self.assets_seeded);
        push_i128(&mut out, self.shares_minted);
        out
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, CodecError> {
        let mut cursor = 0usize;
        if read_u8(bytes, &mut cursor)? != 15 {
            return Err(CodecError::InvalidTag);
        }
        let result = Self {
            assets_seeded: read_i128(bytes, &mut cursor)?,
            shares_minted: read_i128(bytes, &mut cursor)?,
        };
        ensure_finished(bytes, cursor)?;
        Ok(result)
    }
}

/// Byte length of the fixed-size identifiers accepted by the custodial
/// report interface (report integrity hashes and network identifiers).
pub const CUSTODIAL_REPORT_HASH_LEN: usize = 32;

/// Full authenticated custodial valuation report envelope accepted by a
/// constructor-bound custodial adapter.
///
/// The complete envelope is the authorized payload of the adapter
/// submission entrypoint: Soroban account authentication binds exactly
/// these fields, so replaying a report against any other vault, adapter,
/// asset, or network requires fresh authorization from the reporter on
/// that destination. The adapter additionally enforces every domain
/// binding against its constructor configuration and the live ledger
/// network identifier before persisting anything.
#[contracttype]
#[derive(Clone, Debug)]
pub struct CustodialValuationReport {
    /// Vault this valuation is reported for; must match the vault the
    /// adapter is bound to.
    pub vault: soroban_sdk::Address,
    /// Adapter contract authorized to accept this report; must be the
    /// exactly-contract-called custodial adapter.
    pub adapter: soroban_sdk::Address,
    /// Asset whose custodial route value is reported; must match the
    /// asset the adapter is bound to.
    pub asset: soroban_sdk::Address,
    /// Network this valuation is reported on; must equal the SHA-256
    /// hash of the live network passphrase
    /// (`env.ledger().network_id()`).
    pub network_id: soroban_sdk::BytesN<32>,
    /// Monotonic report sequence; must be exactly one greater than the
    /// sequence of the last accepted report for this route. The first
    /// accepted sequence is 1.
    pub sequence: u64,
    /// Valuation time in UTC seconds; must be greater than zero, must
    /// not exceed the ledger timestamp at submission, and must be
    /// strictly greater than the valuation time of the last accepted
    /// report for this route.
    pub as_of: u64,
    /// Reported assets value of the custodial route at `as_of`; must not
    /// be negative.
    pub assets_value: i128,
    /// Optional fixed-size SHA-256 integrity hash of the offchain report
    /// payload. It is persisted and emitted for audit only; the adapter
    /// does not recompute or verify it.
    pub report_hash: Option<soroban_sdk::BytesN<32>>,
}

/// Compact latest-report metadata view consumed by the vault at
/// settlement.
///
/// Fields are, in order: accepted report sequence, valuation `as_of`,
/// ledger timestamp at submission, reported assets value, and optional
/// report integrity hash. Adapters return no view when no settlement-eligible
/// report has been accepted, including after legacy accounting or custody
/// mutations invalidate it. Epoch snapshots can bind the accepted metadata,
/// but the adapter does not verify the offchain hash.
pub type CustodialValuationView = (u64, u64, u64, i128, Option<soroban_sdk::BytesN<32>>);

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::{string::String, vec};
    #[cfg(feature = "immediate-commands")]
    use soroban_sdk::{Address, Env, String as SdkString, Vec as SdkVec};

    #[cfg(feature = "immediate-commands")]
    fn sdk_address(env: &Env) -> Address {
        Address::from_str(
            env,
            "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF",
        )
    }

    #[cfg(any(feature = "immediate-commands", feature = "epoch-commands"))]
    fn receipt_address() -> ReceiptAddress {
        ReceiptAddress::from_str("GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF")
            .expect("valid receipt address")
    }

    #[test]
    fn runtime_feature_bits_are_stable_and_default_mask_matches_public_capabilities() {
        assert_eq!(RUNTIME_FEATURE_ACTION_RECOVERY, 0x01);
        assert_eq!(RUNTIME_FEATURE_ACTION_SYNC_EXTERNAL, 0x02);
        assert_eq!(RUNTIME_FEATURE_ACTION_REFRESH_FEES, 0x04);
        assert_eq!(RUNTIME_FEATURE_ACTION_ALLOCATION_LIFECYCLE, 0x08);
        assert_eq!(RUNTIME_FEATURE_ACTION_REFRESH_LIFECYCLE, 0x10);
        assert_eq!(RUNTIME_FEATURE_ACTION_PAUSE, 0x20);
        assert_eq!(RUNTIME_FEATURE_COMPANION_UPGRADE, 0x40);
        assert_eq!(RUNTIME_FEATURE_ACTION_EPOCH_SETTLEMENT, 0x80);
        assert_eq!(RUNTIME_V1_VERSION, "1.0.0");
        assert_eq!(RUNTIME_V1_FEATURE_FLAGS, 0x3f);
        assert_eq!(RUNTIME_DEFAULT_FEATURE_FLAGS, 0xbf);
        assert_eq!(
            RUNTIME_DEFAULT_FEATURE_FLAGS & RUNTIME_FEATURE_ACTION_PAUSE,
            RUNTIME_FEATURE_ACTION_PAUSE
        );
        assert_eq!(
            RUNTIME_DEFAULT_FEATURE_FLAGS & RUNTIME_FEATURE_ACTION_EPOCH_SETTLEMENT,
            RUNTIME_FEATURE_ACTION_EPOCH_SETTLEMENT
        );
        assert_eq!(
            RUNTIME_DEFAULT_FEATURE_FLAGS & RUNTIME_FEATURE_COMPANION_UPGRADE,
            0
        );
    }

    #[cfg(feature = "immediate-commands")]
    #[test]
    fn vault_command_roundtrip_representative() {
        let commands = vec![
            VaultCommand::DepositWithMin {
                owner: String::from("owner"),
                receiver: String::from("receiver"),
                assets: 100,
                min_shares_out: 1,
            },
            VaultCommand::AtomicWithdraw {
                owner: String::from("owner"),
                receiver: String::from("receiver"),
                operator: String::from("operator"),
                assets: 100,
                max_shares_burned: 101,
            },
            VaultCommand::AtomicRedeem {
                owner: String::from("owner"),
                receiver: String::from("receiver"),
                operator: String::from("operator"),
                shares: 100,
                min_assets_out: 99,
            },
            VaultCommand::ResyncIdleBalance,
            VaultCommand::RefreshFees,
            VaultCommand::CancelMigration {
                caller: String::from("caller"),
            },
            VaultCommand::AbortWithdrawing {
                caller: String::from("caller"),
                op_id: 42,
            },
        ];

        for command in commands {
            let encoded = command.encode();
            let decoded = VaultCommand::decode(&encoded).expect("decode vault command");
            assert_eq!(decoded, command);
        }
    }

    #[cfg(feature = "immediate-commands")]
    #[test]
    fn vault_command_surface_exposes_fee_refresh() {
        let encoded = vec![5];

        assert!(
            VaultCommand::decode(&encoded).is_ok(),
            "VaultCommand has no fee-refresh command tag; persisted fee accrual is unreachable through the deployed ABI"
        );
    }

    #[cfg(feature = "immediate-commands")]
    #[test]
    fn vault_command_decode_rejects_trailing_bytes() {
        let mut encoded = VaultCommand::AtomicWithdraw {
            owner: String::from("owner"),
            receiver: String::from("receiver"),
            operator: String::from("operator"),
            assets: 100,
            max_shares_burned: 101,
        }
        .encode();
        encoded.push(0xFF);

        assert_eq!(
            VaultCommand::decode(&encoded),
            Err(CodecError::InvalidEncoding)
        );
    }
    #[test]
    fn governance_command_roundtrip_representative() {
        let commands = vec![
            GovernanceCommand::SetGovernanceConfig {
                kind: GOVERNANCE_CONFIG_KIND_CURATOR,
                primary: Some(String::from("curator")),
                many: None,
                value_a: None,
                value_b: None,
            },
            GovernanceCommand::SetGovernancePolicy {
                kind: GOVERNANCE_POLICY_KIND_FEES,
                target_ids: None,
                mode: None,
                accounts: Some(vec![String::from("perf"), String::from("mgmt")]),
                market_id: None,
                cap_group_id: None,
                value: Some(11),
                value_b: Some(22),
                value_c: Some(33),
            },
            GovernanceCommand::Skim {
                token: String::from("token"),
            },
        ];

        for command in commands {
            let encoded = command.encode();
            let decoded = GovernanceCommand::decode(&encoded).expect("decode governance command");
            assert_eq!(decoded, command);
        }
    }

    #[test]
    fn governance_command_decode_rejects_trailing_bytes() {
        let mut encoded = GovernanceCommand::Skim {
            token: String::from("token"),
        }
        .encode();
        encoded.push(0xFF);

        assert_eq!(
            GovernanceCommand::decode(&encoded),
            Err(CodecError::InvalidEncoding)
        );
    }

    #[test]
    fn governance_command_decode_rejects_invalid_option_tag() {
        let bytes = vec![GOVERNANCE_COMMAND_TAG_SET_CONFIG, 0, 0, 0, 0, 9];
        assert_eq!(
            GovernanceCommand::decode(&bytes),
            Err(CodecError::InvalidTag)
        );
    }

    #[cfg(feature = "immediate-commands")]
    #[test]
    fn vault_command_decode_rejects_malformed_payloads_by_error_class() {
        let valid = VaultCommand::Allocate {
            caller: String::from("allocator"),
            market: 7,
            amount: 123,
            supply: true,
        }
        .encode();

        assert_eq!(VaultCommand::decode(&[]), Err(CodecError::Truncated));
        assert_eq!(VaultCommand::decode(&[0xFE]), Err(CodecError::InvalidTag));

        let truncated_string = vec![2, 4, 0, 0, 0, b'a', b'b'];
        assert_eq!(
            VaultCommand::decode(&truncated_string),
            Err(CodecError::Truncated)
        );

        let invalid_utf8 = vec![2, 1, 0, 0, 0, 0xFF];
        assert_eq!(
            VaultCommand::decode(&invalid_utf8),
            Err(CodecError::InvalidUtf8)
        );

        let mut invalid_bool = valid.clone();
        *invalid_bool.last_mut().expect("bool byte") = 2;
        assert_eq!(
            VaultCommand::decode(&invalid_bool),
            Err(CodecError::InvalidEncoding)
        );

        let mut trailing = valid;
        trailing.push(0);
        assert_eq!(
            VaultCommand::decode(&trailing),
            Err(CodecError::InvalidEncoding)
        );
    }

    #[test]
    fn governance_command_decode_rejects_incomplete_nested_payloads() {
        let valid = GovernanceCommand::SetGovernancePolicy {
            kind: GOVERNANCE_POLICY_KIND_GROUP,
            target_ids: Some(vec![1, 2]),
            mode: Some(3),
            accounts: Some(vec![String::from("alice"), String::from("bob")]),
            market_id: Some(4),
            cap_group_id: Some(String::from("group")),
            value: Some(5),
            value_b: None,
            value_c: Some(6),
        }
        .encode();

        for len in [0usize, 1, 5, 10, valid.len() - 1] {
            assert_eq!(
                GovernanceCommand::decode(&valid[..len]),
                Err(CodecError::Truncated),
                "length {len} should be rejected as truncated"
            );
        }

        let mut invalid_nested_option = valid.clone();
        // tag + kind + target_ids(Some) + len + two u32s; next byte is mode's option tag.
        invalid_nested_option[1 + 4 + 1 + 4 + 8] = 9;
        assert_eq!(
            GovernanceCommand::decode(&invalid_nested_option),
            Err(CodecError::InvalidTag)
        );

        let mut trailing = valid;
        trailing.extend_from_slice(&[0, 1]);
        assert_eq!(
            GovernanceCommand::decode(&trailing),
            Err(CodecError::InvalidEncoding)
        );
    }

    #[test]
    fn vault_and_governance_tags_do_not_overlap() {
        let governance_commands = vec![
            GovernanceCommand::SetGovernanceConfig {
                kind: GOVERNANCE_CONFIG_KIND_CURATOR,
                primary: Some(String::from("curator")),
                many: None,
                value_a: None,
                value_b: None,
            },
            GovernanceCommand::SetGovernancePolicy {
                kind: GOVERNANCE_POLICY_KIND_FEES,
                target_ids: None,
                mode: None,
                accounts: Some(vec![
                    String::from("performance"),
                    String::from("management"),
                ]),
                market_id: None,
                cap_group_id: None,
                value: Some(11),
                value_b: Some(22),
                value_c: Some(33),
            },
            GovernanceCommand::Skim {
                token: String::from("token"),
            },
        ];

        for governance in governance_commands {
            let encoded = governance.encode();
            assert!(
                VaultCommand::decode(&encoded).is_err(),
                "{governance:?} must not decode as VaultCommand"
            );
        }
    }

    #[cfg(feature = "immediate-commands")]
    #[test]
    fn command_receipts_roundtrip_representative() {
        let status = ExecuteWithdrawStatus {
            op_state_before: 0,
            op_state_after: 2,
            assets_transferred: 1_000,
            events_emitted: 3,
        };

        let deposit = DepositReceipt { shares_out: 12 };
        assert_eq!(
            DepositReceipt::decode(&deposit.encode()).expect("decode deposit receipt"),
            deposit
        );

        let request = RequestWithdrawReceipt {
            request_id: 7,
            shares_escrowed: 34,
        };
        assert_eq!(
            RequestWithdrawReceipt::decode(&request.encode()).expect("decode request receipt"),
            request
        );

        let completed = ExecuteWithdrawReceipt::Completed {
            request_id: 7,
            owner: receipt_address(),
            receiver: receipt_address(),
            assets_out: 21,
            shares_burned: 34,
            status,
        };
        assert_eq!(
            ExecuteWithdrawReceipt::decode(&completed.encode()).expect("decode completed receipt"),
            completed
        );

        let no_payout = ExecuteWithdrawReceipt::NoPayout { status };
        assert_eq!(
            ExecuteWithdrawReceipt::decode(&no_payout.encode()).expect("decode no-payout receipt"),
            no_payout
        );

        let scalar = I128Receipt { value: -5 };
        assert_eq!(
            I128Receipt::decode(&scalar.encode()).expect("decode scalar receipt"),
            scalar
        );

        assert_eq!(
            EmptyReceipt::decode(&EmptyReceipt.encode()).expect("decode empty receipt"),
            EmptyReceipt
        );
    }

    #[cfg(any(feature = "immediate-commands", feature = "epoch-commands"))]
    #[test]
    fn command_receipt_decoders_reject_trailing_bytes() {
        let mut encoded = RequestWithdrawReceipt {
            request_id: 1,
            shares_escrowed: 2,
        }
        .encode();
        encoded.push(0);

        assert_eq!(
            RequestWithdrawReceipt::decode(&encoded),
            Err(CodecError::InvalidEncoding)
        );
    }

    #[cfg(feature = "immediate-commands")]
    #[test]
    fn command_receipt_decoders_reject_wrong_tags() {
        let encoded = I128Receipt { value: 1 }.encode();

        assert_eq!(
            DepositReceipt::decode(&encoded),
            Err(CodecError::InvalidTag)
        );
    }

    #[cfg(any(feature = "immediate-commands", feature = "epoch-commands"))]
    #[test]
    fn command_receipt_decoders_reject_execute_withdraw_wrong_inner_tag() {
        let status = ExecuteWithdrawStatus {
            op_state_before: 0,
            op_state_after: 0,
            assets_transferred: 0,
            events_emitted: 0,
        };
        let mut encoded = ExecuteWithdrawReceipt::NoPayout { status }.encode();
        encoded[1] = 0xFE;

        assert_eq!(
            ExecuteWithdrawReceipt::decode(&encoded),
            Err(CodecError::InvalidTag)
        );
    }

    #[cfg(any(feature = "immediate-commands", feature = "epoch-commands"))]
    #[test]
    fn command_receipt_decoders_reject_truncated_execute_withdraw_completed() {
        let status = ExecuteWithdrawStatus {
            op_state_before: 0,
            op_state_after: 2,
            assets_transferred: 21,
            events_emitted: 3,
        };
        let mut encoded = ExecuteWithdrawReceipt::Completed {
            request_id: 7,
            owner: receipt_address(),
            receiver: receipt_address(),
            assets_out: 21,
            shares_burned: 34,
            status,
        }
        .encode();
        encoded.truncate(encoded.len() - 1);

        assert_eq!(
            ExecuteWithdrawReceipt::decode(&encoded),
            Err(CodecError::Truncated)
        );
    }

    #[cfg(any(feature = "immediate-commands", feature = "epoch-commands"))]
    #[test]
    fn command_receipt_decoders_reject_invalid_execute_withdraw_completed_address() {
        let status = ExecuteWithdrawStatus {
            op_state_before: 0,
            op_state_after: 2,
            assets_transferred: 21,
            events_emitted: 3,
        };
        let mut encoded = ExecuteWithdrawReceipt::Completed {
            request_id: 7,
            owner: receipt_address(),
            receiver: receipt_address(),
            assets_out: 21,
            shares_burned: 34,
            status,
        }
        .encode();
        encoded[14] = b'!';

        assert_eq!(
            ExecuteWithdrawReceipt::decode(&encoded),
            Err(CodecError::InvalidEncoding)
        );
    }

    #[cfg(feature = "immediate-commands")]
    #[test]
    fn proxy_view_fields_map_wire_tuple_positions() {
        let env = Env::default();
        let address = sdk_address(&env);
        let mut queue = SdkVec::new(&env);
        queue.push_back(7);
        let group_id = SdkString::from_str(&env, "senior");
        let mut groups = SdkVec::new(&env);
        groups.push_back((group_id.clone(), 8, 9));

        let fields = ProxyViewFields::from((
            (
                (
                    address.clone(),
                    address.clone(),
                    address.clone(),
                    address.clone(),
                ),
                (10, 11, true),
                (20, 21, 22, 23),
                (30, 31, 32, 33, 34),
            ),
            (queue.clone(), groups.clone()),
            (40, 41, 42, 43, 44, 45, 46, 47),
        ));

        assert_eq!(fields.core.virtual_offsets.virtual_shares, 10);
        assert_eq!(fields.core.virtual_offsets.virtual_assets, 11);
        assert!(fields.core.virtual_offsets.paused);
        assert_eq!(fields.core.totals.total_shares, 20);
        assert_eq!(fields.core.totals.idle_assets, 21);
        assert_eq!(fields.core.totals.external_assets, 22);
        assert_eq!(fields.core.totals.total_assets, 23);
        assert_eq!(fields.core.fees.fee_total_assets, 30);
        assert_eq!(fields.core.fees.fee_timestamp_ns, 31);
        assert_eq!(fields.core.fees.management_fee_wad, 32);
        assert_eq!(fields.core.fees.performance_fee_wad, 33);
        assert_eq!(fields.core.fees.max_total_assets_growth_rate_wad, 34);
        assert!(fields.policy.supply_queue == queue);
        assert!(fields.policy.cap_groups == groups);
        assert_eq!(fields.preview.convert_to_shares, 40);
        assert_eq!(fields.preview.convert_to_assets, 41);
        assert_eq!(fields.preview.max_deposit, 42);
        assert_eq!(fields.preview.max_mint, 43);
        assert_eq!(fields.preview.max_withdraw, 44);
        assert_eq!(fields.preview.max_redeem, 45);
        assert_eq!(fields.preview.preview_mint_assets, 46);
        assert_eq!(fields.preview.preview_withdraw_shares, 47);
    }

    #[cfg(feature = "epoch-commands")]
    #[test]
    fn epoch_settlement_commands_roundtrip() {
        let commands = vec![
            VaultCommand::ConfigureEpochSettlement {
                caller: String::from("governance"),
                max_report_age_ns: 60_000_000_000,
            },
            VaultCommand::RequestDeposit {
                owner: String::from("owner"),
                assets: 120,
                min_shares_out: 100,
            },
            VaultCommand::CancelPendingDeposit {
                owner: String::from("owner"),
                request_id: 3,
            },
            VaultCommand::BeginEpochCutoff {
                caller: String::from("keeper"),
                cutoff_ns: 1_000,
            },
            VaultCommand::SettleEpoch {
                caller: String::from("keeper"),
            },
            VaultCommand::AdmitPendingDeposit {
                caller: String::from("keeper"),
                request_id: 4,
            },
            VaultCommand::CancelPendingWithdrawal {
                owner: String::from("owner"),
                request_id: 5,
            },
            VaultCommand::GetEpochState,
            VaultCommand::GetEpochSnapshot { epoch_id: 1 },
            VaultCommand::GetCustodialReportMetadata { market_id: 7 },
            VaultCommand::SeedEpochSupply {
                caller: String::from("governance"),
                receiver: String::from("treasury"),
                assets: 5_000,
            },
        ];

        for command in commands {
            let encoded = command.encode();
            let decoded = VaultCommand::decode(&encoded).expect("decode epoch settlement command");
            assert_eq!(decoded, command);
        }
    }

    #[cfg(feature = "epoch-commands")]
    #[test]
    fn epoch_settlement_receipts_roundtrip() {
        let receipts: Vec<Vec<u8>> = vec![
            ConfigureEpochSettlementReceipt {
                max_report_age_ns: 60_000_000_000,
            }
            .encode(),
            PendingDepositReceipt {
                request_id: 3,
                assets: 120,
            }
            .encode(),
            CancelPendingDepositReceipt {
                request_id: 3,
                assets_refunded: 120,
            }
            .encode(),
            BeginEpochCutoffReceipt {
                epoch_id: 1,
                cutoff_ns: 1_000,
            }
            .encode(),
            SettleEpochReceipt {
                epoch_id: 1,
                report_seq: 7,
                as_of_ns: 1_000,
                report_hash: [7u8; 32],
                settlement_nav: 1_000_000,
                eligible_supply: 1_000_000,
                cutoff_ns: 1_000,
            }
            .encode(),
            AdmitPendingDepositReceipt {
                request_id: 3,
                shares_out: 119,
                assets_in: 120,
            }
            .encode(),
            CancelPendingWithdrawalReceipt {
                request_id: 5,
                shares_refunded: 40,
                epoch_id: 1,
            }
            .encode(),
            EpochStateViewReceipt {
                phase: EPOCH_PHASE_OPEN,
                intake_epoch: 1,
                cutoff_ns: None,
                last_settled_epoch_id: None,
                last_report_seq: None,
            }
            .encode(),
            EpochStateViewReceipt {
                phase: EPOCH_PHASE_SETTLED,
                intake_epoch: 2,
                cutoff_ns: Some(1_000),
                last_settled_epoch_id: Some(1),
                last_report_seq: Some(7),
            }
            .encode(),
            EpochSnapshotReceipt {
                epoch_id: 1,
                report_seq: 7,
                as_of_ns: 1_000,
                report_hash: [7u8; 32],
                settlement_nav: 1_000_000,
                eligible_supply: 1_000_000,
                cutoff_ns: 1_000,
            }
            .encode(),
            ReportMetadataReceipt::Available {
                market_id: 7,
                seq: 8,
                as_of: 1_000,
                submitted_at: 1_001,
                assets_value: 1_000_000,
                report_hash: Some([8u8; 32]),
            }
            .encode(),
            ReportMetadataReceipt::Available {
                market_id: 7,
                seq: 9,
                as_of: 1_002,
                submitted_at: 1_003,
                assets_value: 1_000_000,
                report_hash: None,
            }
            .encode(),
            ReportMetadataReceipt::Unavailable { market_id: 7 }.encode(),
            SeedEpochSupplyReceipt {
                assets_seeded: 5_000,
                shares_minted: 5_000,
            }
            .encode(),
        ];

        assert_eq!(
            ConfigureEpochSettlementReceipt::decode(&receipts[0]).expect("decode"),
            ConfigureEpochSettlementReceipt {
                max_report_age_ns: 60_000_000_000,
            }
        );
        assert_eq!(
            PendingDepositReceipt::decode(&receipts[1]).expect("decode"),
            PendingDepositReceipt {
                request_id: 3,
                assets: 120,
            }
        );
        assert_eq!(
            CancelPendingDepositReceipt::decode(&receipts[2]).expect("decode"),
            CancelPendingDepositReceipt {
                request_id: 3,
                assets_refunded: 120,
            }
        );
        assert_eq!(
            BeginEpochCutoffReceipt::decode(&receipts[3]).expect("decode"),
            BeginEpochCutoffReceipt {
                epoch_id: 1,
                cutoff_ns: 1_000,
            }
        );
        assert_eq!(
            SettleEpochReceipt::decode(&receipts[4]).expect("decode"),
            SettleEpochReceipt {
                epoch_id: 1,
                report_seq: 7,
                as_of_ns: 1_000,
                report_hash: [7u8; 32],
                settlement_nav: 1_000_000,
                eligible_supply: 1_000_000,
                cutoff_ns: 1_000,
            }
        );
        assert_eq!(
            AdmitPendingDepositReceipt::decode(&receipts[5]).expect("decode"),
            AdmitPendingDepositReceipt {
                request_id: 3,
                shares_out: 119,
                assets_in: 120,
            }
        );
        assert_eq!(
            CancelPendingWithdrawalReceipt::decode(&receipts[6]).expect("decode"),
            CancelPendingWithdrawalReceipt {
                request_id: 5,
                shares_refunded: 40,
                epoch_id: 1,
            }
        );
        assert_eq!(
            EpochStateViewReceipt::decode(&receipts[7]).expect("decode"),
            EpochStateViewReceipt {
                phase: EPOCH_PHASE_OPEN,
                intake_epoch: 1,
                cutoff_ns: None,
                last_settled_epoch_id: None,
                last_report_seq: None,
            }
        );
        assert_eq!(
            EpochStateViewReceipt::decode(&receipts[8]).expect("decode"),
            EpochStateViewReceipt {
                phase: EPOCH_PHASE_SETTLED,
                intake_epoch: 2,
                cutoff_ns: Some(1_000),
                last_settled_epoch_id: Some(1),
                last_report_seq: Some(7),
            }
        );
        assert_eq!(
            EpochSnapshotReceipt::decode(&receipts[9]).expect("decode"),
            EpochSnapshotReceipt {
                epoch_id: 1,
                report_seq: 7,
                as_of_ns: 1_000,
                report_hash: [7u8; 32],
                settlement_nav: 1_000_000,
                eligible_supply: 1_000_000,
                cutoff_ns: 1_000,
            }
        );
        assert_eq!(
            ReportMetadataReceipt::decode(&receipts[10]).expect("decode"),
            ReportMetadataReceipt::Available {
                market_id: 7,
                seq: 8,
                as_of: 1_000,
                submitted_at: 1_001,
                assets_value: 1_000_000,
                report_hash: Some([8u8; 32]),
            }
        );
        assert_eq!(
            ReportMetadataReceipt::decode(&receipts[11]).expect("decode"),
            ReportMetadataReceipt::Available {
                market_id: 7,
                seq: 9,
                as_of: 1_002,
                submitted_at: 1_003,
                assets_value: 1_000_000,
                report_hash: None,
            }
        );
        assert_eq!(
            ReportMetadataReceipt::decode(&receipts[12]).expect("decode"),
            ReportMetadataReceipt::Unavailable { market_id: 7 }
        );
        assert_eq!(
            SeedEpochSupplyReceipt::decode(&receipts[13]).expect("decode"),
            SeedEpochSupplyReceipt {
                assets_seeded: 5_000,
                shares_minted: 5_000,
            }
        );
        assert_eq!(receipts[13][0], 15, "seed receipt tag drifted");
        assert_eq!(
            SeedEpochSupplyReceipt::decode(&EmptyReceipt.encode()),
            Err(CodecError::InvalidTag)
        );
        let mut seed_trailing = receipts[13].clone();
        seed_trailing.extend_from_slice(&[0]);
        assert_eq!(
            SeedEpochSupplyReceipt::decode(&seed_trailing),
            Err(CodecError::InvalidEncoding)
        );
    }

    #[cfg(feature = "epoch-commands")]
    #[test]
    fn epoch_settlement_receipt_decoders_reject_wrong_tags() {
        let empty = EmptyReceipt.encode();
        assert_eq!(
            ConfigureEpochSettlementReceipt::decode(&empty),
            Err(CodecError::InvalidTag)
        );
        assert_eq!(
            PendingDepositReceipt::decode(&empty),
            Err(CodecError::InvalidTag)
        );
        assert_eq!(
            SettleEpochReceipt::decode(&empty),
            Err(CodecError::InvalidTag)
        );
        assert_eq!(
            EpochStateViewReceipt::decode(&empty),
            Err(CodecError::InvalidTag)
        );
        assert_eq!(
            EpochSnapshotReceipt::decode(&empty),
            Err(CodecError::InvalidTag)
        );
        assert_eq!(
            ReportMetadataReceipt::decode(&empty),
            Err(CodecError::InvalidTag)
        );

        let mut wrong_inner = ReportMetadataReceipt::Unavailable { market_id: 7 }.encode();
        wrong_inner[1] = 9;
        assert_eq!(
            ReportMetadataReceipt::decode(&wrong_inner),
            Err(CodecError::InvalidTag)
        );
    }

    #[cfg(feature = "epoch-commands")]
    #[test]
    fn epoch_settlement_codecs_reject_unknown_phase_and_trailing_bytes() {
        let unknown_phase = vec![12, 9, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0];
        assert_eq!(
            EpochStateViewReceipt::decode(&unknown_phase),
            Err(CodecError::InvalidEncoding)
        );

        let mut trailing = PendingDepositReceipt {
            request_id: 1,
            assets: 2,
        }
        .encode();
        trailing.push(0);
        assert_eq!(
            PendingDepositReceipt::decode(&trailing),
            Err(CodecError::InvalidEncoding)
        );
    }

    #[cfg(all(test, feature = "immediate-commands", not(feature = "epoch-commands")))]
    mod immediate_only_wire {
        use super::*;
        use alloc::string::String;

        #[test]
        fn immediate_command_tags_match_origin_dev_wire_exactly() {
            let cases: Vec<(VaultCommand, u8)> = vec![
                (
                    VaultCommand::DepositWithMin {
                        owner: String::from("owner"),
                        receiver: String::from("receiver"),
                        assets: 100,
                        min_shares_out: 1,
                    },
                    0,
                ),
                (
                    VaultCommand::RequestWithdraw {
                        owner: String::from("owner"),
                        receiver: String::from("receiver"),
                        shares: 100,
                        min_assets_out: 99,
                    },
                    1,
                ),
                (
                    VaultCommand::ExecuteWithdraw {
                        caller: String::from("caller"),
                    },
                    2,
                ),
                (
                    VaultCommand::Allocate {
                        caller: String::from("allocator"),
                        market: 7,
                        amount: 123,
                        supply: true,
                    },
                    3,
                ),
                (
                    VaultCommand::RefreshMarkets {
                        caller: String::from("operator"),
                        markets: vec![1, 3],
                    },
                    4,
                ),
                (VaultCommand::RefreshFees, 5),
                (
                    VaultCommand::AtomicWithdraw {
                        owner: String::from("owner"),
                        receiver: String::from("receiver"),
                        operator: String::from("operator"),
                        assets: 100,
                        max_shares_burned: 101,
                    },
                    6,
                ),
                (
                    VaultCommand::AtomicRedeem {
                        owner: String::from("owner"),
                        receiver: String::from("receiver"),
                        operator: String::from("operator"),
                        shares: 100,
                        min_assets_out: 99,
                    },
                    7,
                ),
                (VaultCommand::ResyncIdleBalance, 8),
                (
                    VaultCommand::CancelMigration {
                        caller: String::from("governance"),
                    },
                    9,
                ),
                (VaultCommand::ExtendTtl, 10),
                (
                    VaultCommand::AbortWithdrawing {
                        caller: String::from("caller"),
                        op_id: 42,
                    },
                    11,
                ),
            ];
            for (command, tag) in cases {
                let encoded = command.encode();
                assert_eq!(encoded[0], tag, "immediate command tag drifted");
                assert_eq!(VaultCommand::decode(&encoded).expect("decode"), command);
            }
        }

        #[test]
        fn epoch_command_tags_are_rejected_without_reinterpretation() {
            for tag in 12u8..=21 {
                assert_eq!(
                    VaultCommand::decode(&[tag]),
                    Err(CodecError::InvalidTag),
                    "disabled epoch tag {tag} must fail as InvalidTag"
                );
            }
        }
    }

    #[cfg(all(test, feature = "epoch-commands", not(feature = "immediate-commands")))]
    mod epoch_only_wire {
        const _: () = assert!(
            cfg!(feature = "epoch-commands"),
            "epoch profile must explicitly enable epoch-commands"
        );
        const _: () = assert!(
            !cfg!(feature = "immediate-commands"),
            "epoch profile must not admit immediate-commands"
        );
        use super::*;
        use alloc::string::String;

        #[test]
        fn epoch_command_tags_match_shared_wire_exactly() {
            let cases: Vec<(VaultCommand, u8)> = vec![
                (
                    VaultCommand::ConfigureEpochSettlement {
                        caller: String::from("governance"),
                        max_report_age_ns: 60_000_000_000,
                    },
                    12,
                ),
                (
                    VaultCommand::RequestDeposit {
                        owner: String::from("owner"),
                        assets: 120,
                        min_shares_out: 100,
                    },
                    13,
                ),
                (
                    VaultCommand::CancelPendingDeposit {
                        owner: String::from("owner"),
                        request_id: 3,
                    },
                    14,
                ),
                (
                    VaultCommand::BeginEpochCutoff {
                        caller: String::from("keeper"),
                        cutoff_ns: 1_000,
                    },
                    15,
                ),
                (
                    VaultCommand::SettleEpoch {
                        caller: String::from("keeper"),
                    },
                    16,
                ),
                (
                    VaultCommand::AdmitPendingDeposit {
                        caller: String::from("keeper"),
                        request_id: 4,
                    },
                    17,
                ),
                (
                    VaultCommand::CancelPendingWithdrawal {
                        owner: String::from("owner"),
                        request_id: 5,
                    },
                    18,
                ),
                (VaultCommand::GetEpochState, 19),
                (VaultCommand::GetEpochSnapshot { epoch_id: 1 }, 20),
                (
                    VaultCommand::GetCustodialReportMetadata { market_id: 7 },
                    21,
                ),
                (
                    VaultCommand::SeedEpochSupply {
                        caller: String::from("governance"),
                        receiver: String::from("treasury"),
                        assets: 5_000,
                    },
                    22,
                ),
            ];
            for (command, tag) in cases {
                let encoded = command.encode();
                assert_eq!(encoded[0], tag, "epoch command tag drifted");
                assert_eq!(VaultCommand::decode(&encoded).expect("decode"), command);
            }
        }

        #[test]
        fn queued_withdrawal_commands_and_receipts_roundtrip() {
            let cases: Vec<(VaultCommand, u8)> = vec![
                (
                    VaultCommand::RequestWithdraw {
                        owner: String::from("owner"),
                        receiver: String::from("receiver"),
                        shares: 100,
                        min_assets_out: 99,
                    },
                    1,
                ),
                (
                    VaultCommand::ExecuteWithdraw {
                        caller: String::from("caller"),
                    },
                    2,
                ),
                (
                    VaultCommand::AbortWithdrawing {
                        caller: String::from("caller"),
                        op_id: 42,
                    },
                    11,
                ),
            ];
            for (command, tag) in cases {
                let encoded = command.encode();
                assert_eq!(encoded[0], tag, "queued-withdrawal command tag drifted");
                assert_eq!(VaultCommand::decode(&encoded).expect("decode"), command);
            }
            let request = RequestWithdrawReceipt {
                request_id: 7,
                shares_escrowed: 34,
            };
            assert_eq!(request.encode()[0], 1);
            assert_eq!(
                RequestWithdrawReceipt::decode(&request.encode()).expect("decode"),
                request
            );
            let no_payout = ExecuteWithdrawReceipt::NoPayout {
                status: ExecuteWithdrawStatus {
                    op_state_before: 0,
                    op_state_after: 0,
                    assets_transferred: 0,
                    events_emitted: 0,
                },
            };
            assert_eq!(no_payout.encode()[0], 2);
            assert_eq!(
                ExecuteWithdrawReceipt::decode(&no_payout.encode()).expect("decode"),
                no_payout
            );
        }

        #[test]
        fn immediate_only_command_tags_are_rejected_without_reinterpretation() {
            for tag in [0u8, 3, 4, 5, 6, 7, 8, 9, 10] {
                assert_eq!(
                    VaultCommand::decode(&[tag]),
                    Err(CodecError::InvalidTag),
                    "disabled immediate-only tag {tag} must fail as InvalidTag"
                );
            }
        }

        #[test]
        fn epoch_runtime_flags_do_not_renumber_immediate_flags() {
            assert_eq!(RUNTIME_EPOCH_FEATURE_FLAGS, 0xa0);
            assert_eq!(
                RUNTIME_EPOCH_FEATURE_FLAGS,
                RUNTIME_FEATURE_ACTION_PAUSE | RUNTIME_FEATURE_ACTION_EPOCH_SETTLEMENT
            );
            let excluded = RUNTIME_FEATURE_ACTION_RECOVERY
                | RUNTIME_FEATURE_ACTION_SYNC_EXTERNAL
                | RUNTIME_FEATURE_ACTION_REFRESH_FEES
                | RUNTIME_FEATURE_ACTION_ALLOCATION_LIFECYCLE
                | RUNTIME_FEATURE_ACTION_REFRESH_LIFECYCLE
                | RUNTIME_FEATURE_COMPANION_UPGRADE;
            assert_eq!(RUNTIME_EPOCH_FEATURE_FLAGS & excluded, 0);
            assert_eq!(RUNTIME_V1_FEATURE_FLAGS, 0x3f);
            assert_eq!(RUNTIME_DEFAULT_FEATURE_FLAGS, 0xbf);
            assert_eq!(EPOCH_PHASE_OPEN, 0);
            assert_eq!(EPOCH_PHASE_CUTOFF, 1);
            assert_eq!(EPOCH_PHASE_SETTLED, 2);
        }
    }

    #[cfg(all(test, feature = "immediate-commands", feature = "epoch-commands"))]
    mod combined_wire {
        use super::*;
        use alloc::string::String;

        #[test]
        fn combined_build_keeps_every_command_at_its_fixed_tag() {
            let immediate: Vec<(VaultCommand, u8)> = vec![
                (
                    VaultCommand::DepositWithMin {
                        owner: String::from("owner"),
                        receiver: String::from("receiver"),
                        assets: 100,
                        min_shares_out: 1,
                    },
                    0,
                ),
                (
                    VaultCommand::RequestWithdraw {
                        owner: String::from("owner"),
                        receiver: String::from("receiver"),
                        shares: 100,
                        min_assets_out: 99,
                    },
                    1,
                ),
                (
                    VaultCommand::ExecuteWithdraw {
                        caller: String::from("caller"),
                    },
                    2,
                ),
                (
                    VaultCommand::Allocate {
                        caller: String::from("allocator"),
                        market: 7,
                        amount: 123,
                        supply: false,
                    },
                    3,
                ),
                (
                    VaultCommand::RefreshMarkets {
                        caller: String::from("operator"),
                        markets: vec![2],
                    },
                    4,
                ),
                (VaultCommand::RefreshFees, 5),
                (
                    VaultCommand::AtomicWithdraw {
                        owner: String::from("owner"),
                        receiver: String::from("receiver"),
                        operator: String::from("operator"),
                        assets: 100,
                        max_shares_burned: 101,
                    },
                    6,
                ),
                (
                    VaultCommand::AtomicRedeem {
                        owner: String::from("owner"),
                        receiver: String::from("receiver"),
                        operator: String::from("operator"),
                        shares: 100,
                        min_assets_out: 99,
                    },
                    7,
                ),
                (VaultCommand::ResyncIdleBalance, 8),
                (
                    VaultCommand::CancelMigration {
                        caller: String::from("governance"),
                    },
                    9,
                ),
                (VaultCommand::ExtendTtl, 10),
                (
                    VaultCommand::AbortWithdrawing {
                        caller: String::from("caller"),
                        op_id: 7,
                    },
                    11,
                ),
            ];
            let epoch: Vec<(VaultCommand, u8)> = vec![
                (
                    VaultCommand::ConfigureEpochSettlement {
                        caller: String::from("governance"),
                        max_report_age_ns: 60_000_000_000,
                    },
                    12,
                ),
                (
                    VaultCommand::RequestDeposit {
                        owner: String::from("owner"),
                        assets: 120,
                        min_shares_out: 100,
                    },
                    13,
                ),
                (
                    VaultCommand::CancelPendingDeposit {
                        owner: String::from("owner"),
                        request_id: 3,
                    },
                    14,
                ),
                (
                    VaultCommand::BeginEpochCutoff {
                        caller: String::from("keeper"),
                        cutoff_ns: 1_000,
                    },
                    15,
                ),
                (
                    VaultCommand::SettleEpoch {
                        caller: String::from("keeper"),
                    },
                    16,
                ),
                (
                    VaultCommand::AdmitPendingDeposit {
                        caller: String::from("keeper"),
                        request_id: 4,
                    },
                    17,
                ),
                (
                    VaultCommand::CancelPendingWithdrawal {
                        owner: String::from("owner"),
                        request_id: 5,
                    },
                    18,
                ),
                (VaultCommand::GetEpochState, 19),
                (VaultCommand::GetEpochSnapshot { epoch_id: 2 }, 20),
                (
                    VaultCommand::GetCustodialReportMetadata { market_id: 7 },
                    21,
                ),
                (
                    VaultCommand::SeedEpochSupply {
                        caller: String::from("governance"),
                        receiver: String::from("treasury"),
                        assets: 5_000,
                    },
                    22,
                ),
            ];
            assert_eq!(RUNTIME_EPOCH_FEATURE_FLAGS, 0xa0);
            assert_eq!(RUNTIME_V1_FEATURE_FLAGS, 0x3f);
            assert_eq!(RUNTIME_DEFAULT_FEATURE_FLAGS, 0xbf);
            for (command, tag) in immediate.into_iter().chain(epoch) {
                let encoded = command.encode();
                assert_eq!(encoded[0], tag, "command tag drifted in combined build");
                assert_eq!(VaultCommand::decode(&encoded).expect("decode"), command);
            }
        }
    }
}
