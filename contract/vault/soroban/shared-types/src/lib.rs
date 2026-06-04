#![no_std]

extern crate alloc;

use alloc::{string::String, vec::Vec};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CodecError {
    Truncated,
    InvalidUtf8,
    InvalidTag,
    InvalidEncoding,
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
    AbortWithdrawing {
        caller: String,
        op_id: u64,
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
    RefreshFees,
    ResyncIdleBalance,
    CancelMigration {
        caller: String,
    },
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

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum VaultCommandResult {
    Unit,
    I128(i128),
    U64(u64),
    ExecuteWithdrawStatus(ExecuteWithdrawStatus),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ExecuteWithdrawStatus {
    pub op_state_before: u32,
    pub op_state_after: u32,
    pub assets_transferred: u128,
    pub events_emitted: u32,
}

pub const GOVERNANCE_CONFIG_KIND_CURATOR: u32 = 0;
pub const GOVERNANCE_CONFIG_KIND_GOVERNANCE: u32 = 1;
pub const GOVERNANCE_CONFIG_KIND_SENTINEL: u32 = 2;
pub const GOVERNANCE_CONFIG_KIND_ALLOCATORS: u32 = 4;
pub const GOVERNANCE_CONFIG_KIND_ALLOWED_ADAPTERS: u32 = 5;
pub const GOVERNANCE_CONFIG_KIND_SKIM_RECIPIENT: u32 = 6;
pub const GOVERNANCE_CONFIG_KIND_VIRTUAL_OFFSETS: u32 = 7;
pub const GOVERNANCE_CONFIG_KIND_WITHDRAWAL_COOLDOWN: u32 = 8;

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
            Self::AbortWithdrawing { caller, op_id } => {
                push_u8(&mut out, 11);
                push_string(&mut out, caller);
                push_u64(&mut out, *op_id);
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
            Self::RefreshFees => push_u8(&mut out, 5),
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
        let command = match read_u8(bytes, &mut cursor)? {
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
            11 => Ok(Self::AbortWithdrawing {
                caller: read_string(bytes, &mut cursor)?,
                op_id: read_u64(bytes, &mut cursor)?,
            }),
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
            4 => Ok(Self::RefreshMarkets {
                caller: read_string(bytes, &mut cursor)?,
                markets: read_u32_vec(bytes, &mut cursor)?,
            }),
            5 => Ok(Self::RefreshFees),
            8 => Ok(Self::ResyncIdleBalance),
            9 => Ok(Self::CancelMigration {
                caller: read_string(bytes, &mut cursor)?,
            }),
            10 => Ok(Self::ExtendTtl),
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
            Self::ExecuteWithdrawStatus(status) => {
                push_u8(&mut out, 3);
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
        let result = match read_u8(bytes, &mut cursor)? {
            0 => Ok(Self::Unit),
            1 => Ok(Self::I128(read_i128(bytes, &mut cursor)?)),
            2 => Ok(Self::U64(read_u64(bytes, &mut cursor)?)),
            3 => Ok(Self::ExecuteWithdrawStatus(ExecuteWithdrawStatus {
                op_state_before: read_u32(bytes, &mut cursor)?,
                op_state_after: read_u32(bytes, &mut cursor)?,
                assets_transferred: read_u128(bytes, &mut cursor)?,
                events_emitted: read_u32(bytes, &mut cursor)?,
            })),
            _ => Err(CodecError::InvalidTag),
        }?;
        ensure_finished(bytes, cursor)?;
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::{string::String, vec};

    #[test]
    fn vault_command_roundtrip_representative() {
        let commands = vec![
            VaultCommand::DepositWithMin {
                owner: String::from("owner"),
                receiver: String::from("receiver"),
                assets: 100,
                min_shares_out: 1,
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

    #[test]
    fn vault_command_surface_exposes_fee_refresh() {
        let encoded = vec![5];

        assert!(
            VaultCommand::decode(&encoded).is_ok(),
            "VaultCommand has no fee-refresh command tag; persisted fee accrual is unreachable through the deployed ABI"
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

    #[test]
    fn vault_command_result_roundtrip_execute_withdraw_status() {
        let result = VaultCommandResult::ExecuteWithdrawStatus(ExecuteWithdrawStatus {
            op_state_before: 0,
            op_state_after: 2,
            assets_transferred: 1_000,
            events_emitted: 3,
        });

        let encoded = result.encode();
        let decoded = VaultCommandResult::decode(&encoded).expect("decode command result");
        assert_eq!(decoded, result);
    }
}
