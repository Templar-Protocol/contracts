use std::str::FromStr;

use stellar_strkey::{Contract, Strkey};

use crate::domain::SHARED_DECIMALS;
use crate::error::{Error, Result};

pub const BYTES32_LEN: usize = 32;
pub const EVM_ADDRESS_LEN: usize = 20;

pub const OPTIONS_TYPE_3: u8 = 3;
pub const EXECUTOR_WORKER_ID: u8 = 1;
pub const EXECUTOR_OPTION_TYPE_LZRECEIVE: u8 = 1;
pub const EXECUTOR_OPTION_TYPE_NATIVE_DROP: u8 = 2;

/// Executor-native value delivered to `lzReceive` as `msg.value`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NativeDrop {
    pub amount: u128,
    pub receiver: [u8; BYTES32_LEN],
}

/// Type-3 receive options restricted to the executor surface this CLI builds:
/// mandatory `lzReceive` gas plus at most one native drop.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Type3Options {
    pub gas: u128,
    pub native_drop: Option<NativeDrop>,
}

// ---------------------------------------------------------------------------
// StrKey <-> bytes32
// ---------------------------------------------------------------------------

/// Decodes a Stellar classic account (`G...`) or contract (`C...`) StrKey,
/// verifying checksum and length, into its 32-byte LayerZero peer bytes.
pub fn strkey_to_bytes32(strkey: &str) -> Result<[u8; BYTES32_LEN]> {
    match Strkey::from_str(strkey) {
        Ok(Strkey::PublicKeyEd25519(key)) => Ok(key.0),
        Ok(Strkey::Contract(id)) => Ok(id.0),
        Ok(_) => Err(Error::InvalidInput(format!(
            "strkey {strkey} is not a classic account or contract address"
        ))),
        Err(error) => Err(Error::InvalidInput(format!(
            "malformed strkey {strkey}: {error}"
        ))),
    }
}

/// Encodes 32 bytes as a classic account (`G...`) StrKey. Inherent
/// `to_string` on 0.0.16 returns a heapless string, so encoding goes through
/// `Display` to produce an owned std string.
pub fn bytes32_to_strkey_account(bytes: &[u8; BYTES32_LEN]) -> Result<String> {
    Ok(format!(
        "{}",
        Strkey::PublicKeyEd25519(stellar_strkey::ed25519::PublicKey(*bytes))
    ))
}

/// Encodes 32 bytes as a contract (`C...`) StrKey.
pub fn bytes32_to_strkey_contract(bytes: &[u8; BYTES32_LEN]) -> Result<String> {
    Ok(format!("{}", Strkey::Contract(Contract(*bytes))))
}

// ---------------------------------------------------------------------------
// Stellar contract address derivation (official XDR preimage rules)
// ---------------------------------------------------------------------------

/// Hashes an official `HashIdPreimage` to the 32-byte contract identifier.
fn contract_id_from_preimage(preimage: &stellar_baselib::xdr::HashIdPreimage) -> Result<[u8; 32]> {
    use sha2::Digest as _;
    use stellar_baselib::xdr::{Limits, WriteXdr};
    let encoded = preimage
        .to_xdr(Limits::none())
        .map_err(|error| Error::InvalidInput(format!("xdr encode failed: {error}")))?;
    let digest = sha2::Sha256::digest(encoded);
    let mut bytes = [0u8; 32];
    bytes.copy_from_slice(&digest);
    Ok(bytes)
}

/// Deterministic native SAC contract identifier for `passphrase`:
/// `SHA-256(HashIdPreimage::ContractId { network_id, Asset::Native })`.
pub fn derive_native_sac_contract(passphrase: &str) -> Result<String> {
    use sha2::Digest as _;
    use stellar_baselib::xdr::{
        Asset, ContractIdPreimage, Hash, HashIdPreimage, HashIdPreimageContractId,
    };
    if passphrase.trim().is_empty() {
        return Err(Error::InvalidInput(
            "network passphrase must not be empty".into(),
        ));
    }
    let network_id: [u8; 32] = sha2::Sha256::digest(passphrase.as_bytes()).into();
    let id = contract_id_from_preimage(&HashIdPreimage::ContractId(HashIdPreimageContractId {
        network_id: Hash(network_id),
        contract_id_preimage: ContractIdPreimage::Asset(Asset::Native),
    }))?;
    bytes32_to_strkey_contract(&id)
}

/// Deterministic `create_contract_v2` address for `deployer` (`G...`) and a
/// 32-byte salt: `SHA-256(HashIdPreimage::ContractId { network_id,
/// FromAddress(ScAddress::Account(deployer), salt) })`.
pub fn derive_stellar_contract_address(
    passphrase: &str,
    deployer: &str,
    salt: &[u8; 32],
) -> Result<String> {
    use sha2::Digest as _;
    use stellar_baselib::xdr::{
        AccountId, ContractIdPreimage, ContractIdPreimageFromAddress, Hash, HashIdPreimage,
        HashIdPreimageContractId, PublicKey, ScAddress, Uint256,
    };
    let deployer_bytes = match Strkey::from_str(deployer) {
        Ok(Strkey::PublicKeyEd25519(key)) => key.0,
        _ => {
            return Err(Error::InvalidInput(
                "deployer must be a classic Stellar account (G...)".into(),
            ))
        }
    };
    let network_id: [u8; 32] = sha2::Sha256::digest(passphrase.as_bytes()).into();
    let id = contract_id_from_preimage(&HashIdPreimage::ContractId(HashIdPreimageContractId {
        network_id: Hash(network_id),
        contract_id_preimage: ContractIdPreimage::Address(ContractIdPreimageFromAddress {
            address: ScAddress::Account(AccountId(PublicKey::PublicKeyTypeEd25519(Uint256(
                deployer_bytes,
            )))),
            salt: Uint256(*salt),
        }),
    }))?;
    bytes32_to_strkey_contract(&id)
}

// ---------------------------------------------------------------------------
// EVM address <-> bytes32
// ---------------------------------------------------------------------------

/// Parses a 20-byte `0x`-prefixed EVM address and left-pads it into the
/// 32-byte LayerZero peer bytes.
pub fn evm_address_to_bytes32(address: &str) -> Result<[u8; BYTES32_LEN]> {
    let hex_digits = address
        .strip_prefix("0x")
        .ok_or_else(|| Error::InvalidInput(format!("EVM address {address} lacks 0x prefix")))?;
    if hex_digits.len() != EVM_ADDRESS_LEN * 2 {
        return Err(Error::InvalidInput(format!(
            "EVM address {address} is not {} hex digits",
            EVM_ADDRESS_LEN * 2
        )));
    }
    let raw = hex::decode(hex_digits).map_err(|error| {
        Error::InvalidInput(format!("malformed EVM address {address}: {error}"))
    })?;
    let mut bytes = [0u8; BYTES32_LEN];
    bytes[BYTES32_LEN - EVM_ADDRESS_LEN..].copy_from_slice(&raw);
    Ok(bytes)
}

/// Recovers the lowercase `0x`-prefixed EVM address from left-padded peer
/// bytes, refusing bytes whose upper 12 octets are not zero.
pub fn bytes32_to_evm_address(bytes: &[u8; BYTES32_LEN]) -> Result<String> {
    if !bytes[..BYTES32_LEN - EVM_ADDRESS_LEN]
        .iter()
        .all(|&b| b == 0)
    {
        return Err(Error::InvalidInput(
            "peer bytes are not a left-padded EVM address".into(),
        ));
    }
    Ok(format!(
        "0x{}",
        hex::encode(&bytes[BYTES32_LEN - EVM_ADDRESS_LEN..])
    ))
}

// ---------------------------------------------------------------------------
// Shared decimal conversion
// ---------------------------------------------------------------------------

/// Converts local token raw units into shared (6-decimal) raw units using
/// checked integer arithmetic, rejecting dust below the shared precision.
pub fn to_shared(amount_local: u128, local_decimals: u8) -> Result<u128> {
    let factor = shared_scaling_factor(local_decimals)?;
    if amount_local % factor != 0 {
        return Err(Error::InvalidInput(format!(
            "amount {amount_local} has dust below shared precision {SHARED_DECIMALS}"
        )));
    }
    Ok(amount_local / factor)
}

/// Converts shared (6-decimal) raw units back into local token raw units
/// using checked integer arithmetic.
pub fn from_shared(amount_shared: u128, local_decimals: u8) -> Result<u128> {
    let factor = shared_scaling_factor(local_decimals)?;
    amount_shared
        .checked_mul(factor)
        .ok_or_else(|| Error::InvalidInput("shared amount overflows local raw units".into()))
}

fn shared_scaling_factor(local_decimals: u8) -> Result<u128> {
    if local_decimals < SHARED_DECIMALS {
        return Err(Error::InvalidInput(format!(
            "local decimals {local_decimals} are below shared decimals {SHARED_DECIMALS}"
        )));
    }
    10u128
        .checked_pow(u32::from(local_decimals - SHARED_DECIMALS))
        .ok_or_else(|| {
            Error::InvalidInput(format!(
                "local decimals {local_decimals} overflow the scaling factor"
            ))
        })
}

// ---------------------------------------------------------------------------
// Type-3 options
// ---------------------------------------------------------------------------

/// Encodes Type-3 executor receive options.
///
/// Wire format per official `message-lib-common`:
/// `[0x03][worker_id:u8][option_size:u16 be][option_data]` where executor
/// `lzReceive` data is `[0x01][gas:u128 be]` and native drop data is
/// `[0x02][amount:u128 be][receiver:32]`.
pub fn encode_type3_options(options: &Type3Options) -> Result<Vec<u8>> {
    if options.gas == 0 {
        return Err(Error::InvalidInput(
            "lzReceive gas must be greater than zero".into(),
        ));
    }
    if let Some(drop) = options.native_drop {
        if drop.amount == 0 {
            return Err(Error::InvalidInput(
                "native drop amount must be greater than zero".into(),
            ));
        }
    }
    let mut bytes = Vec::with_capacity(1 + 3 + 17 + options.native_drop.map_or(0, |_| 3 + 49));
    bytes.push(OPTIONS_TYPE_3);
    push_lz_receive_option(&mut bytes, options.gas);
    if let Some(drop) = options.native_drop {
        push_native_drop_option(&mut bytes, drop);
    }
    Ok(bytes)
}

const LZRECEIVE_OPTION_SIZE: u16 = 1 + 16;
const NATIVE_DROP_OPTION_SIZE: u16 = 1 + 16 + 32; // BYTES32_LEN as u16 (const-stable form)

fn push_lz_receive_option(bytes: &mut Vec<u8>, gas: u128) {
    let mut option = [0u8; 3 + 17];
    option[0] = EXECUTOR_WORKER_ID;
    option[1..3].copy_from_slice(&LZRECEIVE_OPTION_SIZE.to_be_bytes());
    option[3] = EXECUTOR_OPTION_TYPE_LZRECEIVE;
    option[4..].copy_from_slice(&gas.to_be_bytes());
    bytes.extend_from_slice(&option);
}

fn push_native_drop_option(bytes: &mut Vec<u8>, drop: NativeDrop) {
    let mut option = [0u8; 3 + 49];
    option[0] = EXECUTOR_WORKER_ID;
    option[1..3].copy_from_slice(&NATIVE_DROP_OPTION_SIZE.to_be_bytes());
    option[3] = EXECUTOR_OPTION_TYPE_NATIVE_DROP;
    option[4..20].copy_from_slice(&drop.amount.to_be_bytes());
    option[20..].copy_from_slice(&drop.receiver);
    bytes.extend_from_slice(&option);
}

/// Decodes Type-3 executor receive options with strict length and type
/// checks: exactly one `lzReceive` option, at most one native drop, no
/// DVN options, and no trailing bytes.
pub fn decode_type3_options(bytes: &[u8]) -> Result<Type3Options> {
    let Some((&OPTIONS_TYPE_3, rest)) = bytes.split_first() else {
        return Err(Error::InvalidInput(
            "options do not start with the Type-3 marker".into(),
        ));
    };
    let mut cursor = rest;
    let mut gas = None;
    let mut native_drop = None;
    while !cursor.is_empty() {
        if cursor.len() < 3 {
            return Err(Error::InvalidInput("truncated worker option header".into()));
        }
        let worker_id = cursor[0];
        let option_size = usize::from(u16::from_be_bytes([cursor[1], cursor[2]]));
        cursor = &cursor[3..];
        if cursor.len() < option_size {
            return Err(Error::InvalidInput("truncated worker option data".into()));
        }
        let data = &cursor[..option_size];
        cursor = &cursor[option_size..];
        match worker_id {
            EXECUTOR_WORKER_ID => {
                let Some((&option_type, payload)) = data.split_first() else {
                    return Err(Error::InvalidInput("empty executor option".into()));
                };
                match option_type {
                    EXECUTOR_OPTION_TYPE_LZRECEIVE => {
                        if gas.is_some() {
                            return Err(Error::InvalidInput(
                                "duplicate lzReceive executor option".into(),
                            ));
                        }
                        let payload: [u8; 16] = payload.try_into().map_err(|_| {
                            Error::InvalidInput("lzReceive option must carry u128 gas".into())
                        })?;
                        gas = Some(u128::from_be_bytes(payload));
                    }
                    EXECUTOR_OPTION_TYPE_NATIVE_DROP => {
                        if native_drop.is_some() {
                            return Err(Error::InvalidInput(
                                "duplicate native drop executor option".into(),
                            ));
                        }
                        let payload: [u8; 16 + BYTES32_LEN] = payload.try_into().map_err(|_| {
                            Error::InvalidInput(
                                "native drop option must carry u128 amount and 32-byte receiver"
                                    .into(),
                            )
                        })?;
                        let amount_bytes: [u8; 16] = payload[..16].try_into().map_err(|_| {
                            Error::InvalidInput("native drop amount must be u128".into())
                        })?;
                        let receiver: [u8; BYTES32_LEN] =
                            payload[16..].try_into().map_err(|_| {
                                Error::InvalidInput("native drop receiver must be bytes32".into())
                            })?;
                        let amount = u128::from_be_bytes(amount_bytes);
                        native_drop = Some(NativeDrop { amount, receiver });
                    }
                    other => {
                        return Err(Error::InvalidInput(format!(
                            "unknown executor option type {other}"
                        )));
                    }
                }
            }
            other => {
                return Err(Error::InvalidInput(format!(
                    "unsupported worker id {other} in Type-3 options"
                )));
            }
        }
    }
    Ok(Type3Options {
        gas: gas.ok_or_else(|| Error::InvalidInput("missing lzReceive executor option".into()))?,
        native_drop,
    })
}
