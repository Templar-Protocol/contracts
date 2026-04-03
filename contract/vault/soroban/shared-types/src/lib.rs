#![no_std]

extern crate alloc;

use alloc::{string::String, vec::Vec};

#[derive(Clone, Eq, PartialEq)]
pub enum CodecError {
    Truncated,
    InvalidUtf8,
    InvalidTag,
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
    let mut values = Vec::with_capacity(len);
    for _ in 0..len {
        values.push(read_u32(bytes, cursor)?);
    }
    Ok(values)
}

fn read_string_vec(bytes: &[u8], cursor: &mut usize) -> Result<Vec<String>, CodecError> {
    let len = read_u32(bytes, cursor)? as usize;
    let mut values = Vec::with_capacity(len);
    for _ in 0..len {
        values.push(read_string(bytes, cursor)?);
    }
    Ok(values)
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

#[derive(Clone, Eq, PartialEq)]
pub enum VaultCommand {
    DepositWithMin {
        owner: String,
        receiver: String,
        assets: i128,
        min_shares_out: i128,
    },
    RequestWithdraw {
        owner: String,
        receiver: String,
        shares: i128,
        min_assets_out: i128,
    },
    ExecuteWithdraw {
        caller: String,
    },
    Allocate {
        caller: String,
        market: u32,
        amount: i128,
        supply: bool,
    },
    RefreshMarkets {
        caller: String,
        markets: Vec<u32>,
    },
    SetGovernanceConfig {
        caller: String,
        kind: u32,
        primary: Option<String>,
        many: Option<Vec<String>>,
        value_a: Option<i128>,
        value_b: Option<i128>,
    },
    SetGovernancePolicy {
        caller: String,
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
        caller: String,
        token: String,
    },
    ResyncIdleBalance,
    CancelMigration {
        caller: String,
    },
    ExtendTtl,
}

#[derive(Clone, Eq, PartialEq)]
pub enum VaultCommandResult {
    Unit,
    I128(i128),
    U64(u64),
}

pub const GOVERNANCE_CONFIG_KIND_CURATOR: u32 = 0;
pub const GOVERNANCE_CONFIG_KIND_GOVERNANCE: u32 = 1;
pub const GOVERNANCE_CONFIG_KIND_SENTINEL: u32 = 2;
pub const GOVERNANCE_CONFIG_KIND_GUARDIANS: u32 = 3;
pub const GOVERNANCE_CONFIG_KIND_ALLOCATORS: u32 = 4;
pub const GOVERNANCE_CONFIG_KIND_ALLOWED_ADAPTERS: u32 = 5;
pub const GOVERNANCE_CONFIG_KIND_SKIM_RECIPIENT: u32 = 6;
pub const GOVERNANCE_CONFIG_KIND_VIRTUAL_OFFSETS: u32 = 7;

pub const GOVERNANCE_POLICY_KIND_SUPPLY_QUEUE: u32 = 0;
pub const GOVERNANCE_POLICY_KIND_CAP: u32 = 1;
pub const GOVERNANCE_POLICY_KIND_REMOVE_MARKET: u32 = 2;
pub const GOVERNANCE_POLICY_KIND_RESTRICTIONS: u32 = 3;
pub const GOVERNANCE_POLICY_KIND_GROUP: u32 = 4;
pub const GOVERNANCE_POLICY_KIND_PAUSED: u32 = 5;
pub const GOVERNANCE_POLICY_KIND_FEES: u32 = 6;

impl VaultCommand {
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        match self {
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
            Self::ExecuteWithdraw { caller } => {
                push_u8(&mut out, 2);
                push_string(&mut out, caller);
            }
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
            Self::RefreshMarkets { caller, markets } => {
                push_u8(&mut out, 4);
                push_string(&mut out, caller);
                push_u32_vec(&mut out, markets);
            }
            Self::SetGovernanceConfig {
                caller,
                kind,
                primary,
                many,
                value_a,
                value_b,
            } => {
                push_u8(&mut out, 5);
                push_string(&mut out, caller);
                push_u32(&mut out, *kind);
                push_option_string(&mut out, primary);
                push_option_string_vec(&mut out, many);
                push_option_i128(&mut out, value_a);
                push_option_i128(&mut out, value_b);
            }
            Self::SetGovernancePolicy {
                caller,
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
                push_u8(&mut out, 6);
                push_string(&mut out, caller);
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
            Self::Skim { caller, token } => {
                push_u8(&mut out, 7);
                push_string(&mut out, caller);
                push_string(&mut out, token);
            }
            Self::ResyncIdleBalance => push_u8(&mut out, 8),
            Self::CancelMigration { caller } => {
                push_u8(&mut out, 9);
                push_string(&mut out, caller);
            }
            Self::ExtendTtl => push_u8(&mut out, 10),
        }
        out
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, CodecError> {
        let mut cursor = 0usize;
        match read_u8(bytes, &mut cursor)? {
            0 => Ok(Self::DepositWithMin {
                owner: read_string(bytes, &mut cursor)?,
                receiver: read_string(bytes, &mut cursor)?,
                assets: read_i128(bytes, &mut cursor)?,
                min_shares_out: read_i128(bytes, &mut cursor)?,
            }),
            1 => Ok(Self::RequestWithdraw {
                owner: read_string(bytes, &mut cursor)?,
                receiver: read_string(bytes, &mut cursor)?,
                shares: read_i128(bytes, &mut cursor)?,
                min_assets_out: read_i128(bytes, &mut cursor)?,
            }),
            2 => Ok(Self::ExecuteWithdraw {
                caller: read_string(bytes, &mut cursor)?,
            }),
            3 => Ok(Self::Allocate {
                caller: read_string(bytes, &mut cursor)?,
                market: read_u32(bytes, &mut cursor)?,
                amount: read_i128(bytes, &mut cursor)?,
                supply: read_u8(bytes, &mut cursor)? != 0,
            }),
            4 => Ok(Self::RefreshMarkets {
                caller: read_string(bytes, &mut cursor)?,
                markets: read_u32_vec(bytes, &mut cursor)?,
            }),
            5 => Ok(Self::SetGovernanceConfig {
                caller: read_string(bytes, &mut cursor)?,
                kind: read_u32(bytes, &mut cursor)?,
                primary: read_option_string(bytes, &mut cursor)?,
                many: read_option_string_vec(bytes, &mut cursor)?,
                value_a: read_option_i128(bytes, &mut cursor)?,
                value_b: read_option_i128(bytes, &mut cursor)?,
            }),
            6 => Ok(Self::SetGovernancePolicy {
                caller: read_string(bytes, &mut cursor)?,
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
            7 => Ok(Self::Skim {
                caller: read_string(bytes, &mut cursor)?,
                token: read_string(bytes, &mut cursor)?,
            }),
            8 => Ok(Self::ResyncIdleBalance),
            9 => Ok(Self::CancelMigration {
                caller: read_string(bytes, &mut cursor)?,
            }),
            10 => Ok(Self::ExtendTtl),
            _ => Err(CodecError::InvalidTag),
        }
    }
}

impl VaultCommandResult {
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        match self {
            Self::Unit => push_u8(&mut out, 0),
            Self::I128(value) => {
                push_u8(&mut out, 1);
                push_i128(&mut out, *value);
            }
            Self::U64(value) => {
                push_u8(&mut out, 2);
                push_u64(&mut out, *value);
            }
        }
        out
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, CodecError> {
        let mut cursor = 0usize;
        match read_u8(bytes, &mut cursor)? {
            0 => Ok(Self::Unit),
            1 => Ok(Self::I128(read_i128(bytes, &mut cursor)?)),
            2 => Ok(Self::U64(read_u64(bytes, &mut cursor)?)),
            _ => Err(CodecError::InvalidTag),
        }
    }
}
