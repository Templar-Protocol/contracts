//! LayerZero checked native adapter: desired/effective route comparison,
//! typed Type-3 security and executor config, and directional containment
//! plans. Pure decisions over typed inputs; no live chain mutation.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::domain::{DesiredRouteV1, Direction, OperationV1, RouteStateV1, Vm};
use crate::error::{Error, Result};

alloy::sol! {
    struct OftSendParamV1 {
        uint32 dstEid;
        bytes32 to;
        uint256 amountLD;
        uint256 minAmountLD;
        bytes extraOptions;
        bytes composeMsg;
        bytes oftCmd;
    }
    struct SetConfigParamV1 {
        uint32 eid;
        uint32 configType;
        bytes config;
    }
    interface IEndpointConfigV1 {
        function setConfig(
            address oapp,
            address lib,
            SetConfigParamV1[] params
        ) external;
    }
    struct MessagingFeeV1 {
        uint256 nativeFee;
        uint256 lzTokenFee;
    }
    interface IOftSendV1 {
        function send(
            OftSendParamV1 sendParam,
            MessagingFeeV1 fee,
            address refundAddress
        ) external payable;
    }
    struct OriginV1 {
        uint32 srcEid;
        bytes32 sender;
        uint64 nonce;
    }
    interface IEndpointReceiveV1 {
        function lzReceive(
            OriginV1 origin,
            address receiver,
            bytes32 guid,
            bytes message,
            bytes extraData
        ) external payable;
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StellarInvocationV1 {
    pub function: String,
    pub args_xdr_hex: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PacketHeaderV1 {
    pub nonce: u64,
    pub source_eid: u32,
    pub sender: [u8; 32],
    pub destination_eid: u32,
    pub receiver: [u8; 32],
}

pub fn decode_packet_header(value: &str) -> Result<PacketHeaderV1> {
    let bytes = hex::decode(value.trim_start_matches("0x"))
        .map_err(|error| Error::InvalidInput(format!("invalid packet header hex: {error}")))?;
    if bytes.len() != 81 || bytes[0] != 1 {
        return Err(Error::InvalidInput(
            "packet header must be the exact 81-byte LayerZero v1 header".into(),
        ));
    }
    let u64_at = |start: usize| {
        let mut value = [0u8; 8];
        value.copy_from_slice(&bytes[start..start + 8]);
        u64::from_be_bytes(value)
    };
    let u32_at = |start: usize| {
        let mut value = [0u8; 4];
        value.copy_from_slice(&bytes[start..start + 4]);
        u32::from_be_bytes(value)
    };
    let bytes32_at = |start: usize| {
        let mut value = [0u8; 32];
        value.copy_from_slice(&bytes[start..start + 32]);
        value
    };
    Ok(PacketHeaderV1 {
        nonce: u64_at(1),
        source_eid: u32_at(9),
        sender: bytes32_at(13),
        destination_eid: u32_at(45),
        receiver: bytes32_at(49),
    })
}

pub fn qualify_message_for_route(
    state: &RouteStateV1,
    message: &crate::domain::MessageRecordV1,
) -> Result<PacketHeaderV1> {
    use sha2::{Digest as _, Sha256};

    let header = decode_packet_header(&message.packet_header)?;
    let (source_eid, destination_eid, source_vm, destination_vm) = match message.direction {
        Direction::StellarToEvm => (
            state.identity.stellar_eid,
            state.identity.evm_eid,
            Vm::Stellar,
            Vm::Evm,
        ),
        Direction::EvmToStellar => (
            state.identity.evm_eid,
            state.identity.stellar_eid,
            Vm::Evm,
            Vm::Stellar,
        ),
    };
    if message.source_eid != source_eid
        || header.source_eid != source_eid
        || header.destination_eid != destination_eid
        || message.nonce.parse::<u64>().ok() != Some(header.nonce)
    {
        return Err(Error::Custody(
            "packet header eid or nonce differs from the message identity".into(),
        ));
    }
    let sender = hex::decode(message.sender.trim_start_matches("0x"))
        .map_err(|_| Error::Custody("message sender must be bytes32 hex".into()))?;
    if sender.as_slice() != header.sender {
        return Err(Error::Custody(
            "packet header sender differs from the message identity".into(),
        ));
    }
    let source_oft = state
        .contracts
        .get(match source_vm {
            Vm::Stellar => "stellar_oft",
            Vm::Evm => "evm_oft",
        })
        .ok_or_else(|| Error::Custody("source OFT is not recorded".into()))?;
    let expected_sender = match source_vm {
        Vm::Stellar => crate::codec::strkey_to_bytes32(source_oft)?,
        Vm::Evm => crate::codec::evm_address_to_bytes32(source_oft)?,
    };
    if header.sender != expected_sender {
        return Err(Error::Custody(
            "packet sender is not the recorded source OFT".into(),
        ));
    }
    let destination_oft = state
        .contracts
        .get(match destination_vm {
            Vm::Stellar => "stellar_oft",
            Vm::Evm => "evm_oft",
        })
        .ok_or_else(|| Error::Custody("destination OFT is not recorded".into()))?;
    let expected_receiver = match destination_vm {
        Vm::Stellar => crate::codec::strkey_to_bytes32(destination_oft)?,
        Vm::Evm => crate::codec::evm_address_to_bytes32(destination_oft)?,
    };
    if header.receiver != expected_receiver {
        return Err(Error::Custody(
            "packet receiver is not the recorded destination OFT".into(),
        ));
    }
    let guid: [u8; 32] = hex::decode(message.guid.trim_start_matches("0x"))
        .map_err(|_| Error::Custody("message guid is not hex".into()))?
        .try_into()
        .map_err(|_| Error::Custody("message guid must be 32 bytes".into()))?;
    let body = hex::decode(message.message.trim_start_matches("0x"))
        .map_err(|_| Error::Custody("message body is not hex".into()))?;
    let mut payload = Vec::with_capacity(32 + body.len());
    payload.extend(guid);
    payload.extend(&body);
    if !hex::encode(crate::evm::keccak256_of(&payload))
        .eq_ignore_ascii_case(message.payload_keccak256.trim_start_matches("0x"))
    {
        return Err(Error::Custody(
            "payload_keccak256 differs from keccak256(guid || message)".into(),
        ));
    }
    let packet_header = hex::decode(message.packet_header.trim_start_matches("0x"))
        .map_err(|_| Error::Custody("message packet header is not hex".into()))?;
    let mut packet = Vec::with_capacity(packet_header.len() + payload.len());
    packet.extend(packet_header);
    packet.extend(payload);
    if !hex::encode(Sha256::digest(packet))
        .eq_ignore_ascii_case(message.packet_sha256.trim_start_matches("0x"))
    {
        return Err(Error::Custody(
            "packet_sha256 differs from the exact encoded packet".into(),
        ));
    }
    Ok(header)
}

/// Encodes one Soroban argument after all exact fields are represented by `OperationV1`.
fn encode_stellar_scval(value: stellar_baselib::xdr::ScVal) -> Result<String> {
    use stellar_baselib::xdr::{Limits, WriteXdr as _};
    Ok(hex::encode(value.to_xdr(Limits::none()).map_err(
        |error| Error::InvalidInput(format!("xdr encode failed: {error}")),
    )?))
}

fn stellar_i128_parts(value: i128) -> stellar_baselib::xdr::Int128Parts {
    let bytes = value.to_be_bytes();
    stellar_baselib::xdr::Int128Parts {
        hi: i64::from_be_bytes([
            bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
        ]),
        lo: u64::from_be_bytes([
            bytes[8], bytes[9], bytes[10], bytes[11], bytes[12], bytes[13], bytes[14], bytes[15],
        ]),
    }
}

pub(crate) fn stellar_address(value: &str) -> Result<stellar_baselib::xdr::ScVal> {
    use std::str::FromStr as _;
    use stellar_baselib::xdr::{AccountId, ContractId, Hash, PublicKey, ScAddress, ScVal, Uint256};
    use stellar_strkey::Strkey;


    let address = match Strkey::from_str(value) {
        Ok(Strkey::PublicKeyEd25519(key)) => {
            ScAddress::Account(AccountId(PublicKey::PublicKeyTypeEd25519(Uint256(key.0))))
        }
        Ok(Strkey::Contract(contract)) => ScAddress::Contract(ContractId(Hash(contract.0))),
        _ => {
            return Err(Error::InvalidInput(format!(
                "invalid Stellar address: {value}"
            )))
        }
    };
    Ok(ScVal::Address(address))
}

fn stellar_set_config_invocation(
    caller: &str,
    oapp: &str,
    library: &str,
    remote_eid: u32,
    config_type: u32,
    config: Vec<u8>,
) -> Result<StellarInvocationV1> {
    use stellar_baselib::xdr::{ScBytes, ScMap, ScMapEntry, ScSymbol, ScVal, ScVec, StringM, VecM};
    let symbol = |value: &str| {
        Ok::<_, Error>(ScVal::Symbol(ScSymbol(
            StringM::try_from(value.as_bytes().to_vec())
                .map_err(|error| Error::InvalidInput(format!("invalid symbol: {error}")))?,
        )))
    };
    let param = ScVal::Map(Some(ScMap(
        VecM::try_from(vec![
            ScMapEntry {
                key: symbol("config")?,
                val: ScVal::Bytes(ScBytes(config.try_into().map_err(|error| {
                    Error::InvalidInput(format!("config bytes too large: {error}"))
                })?)),
            },
            ScMapEntry {
                key: symbol("config_type")?,
                val: ScVal::U32(config_type),
            },
            ScMapEntry {
                key: symbol("eid")?,
                val: ScVal::U32(remote_eid),
            },
        ])
        .map_err(|error| Error::InvalidInput(format!("config map too large: {error}")))?,
    )));
    Ok(StellarInvocationV1 {
        function: "set_config".into(),
        args_xdr_hex: vec![
            encode_stellar_scval(stellar_address(caller)?)?,
            encode_stellar_scval(stellar_address(oapp)?)?,
            encode_stellar_scval(stellar_address(library)?)?,
            encode_stellar_scval(ScVal::Vec(Some(ScVec(
                VecM::try_from(vec![param]).map_err(|error| {
                    Error::InvalidInput(format!("config vector too large: {error}"))
                })?,
            ))))?,
        ],
    })
}

fn stellar_symbol(value: &str) -> Result<stellar_baselib::xdr::ScVal> {
    use stellar_baselib::xdr::{ScSymbol, ScVal, StringM};
    Ok(ScVal::Symbol(ScSymbol(
        StringM::try_from(value.as_bytes().to_vec())
            .map_err(|error| Error::InvalidInput(format!("invalid symbol: {error}")))?,
    )))
}

fn stellar_bytes(value: Vec<u8>) -> Result<stellar_baselib::xdr::ScVal> {
    use stellar_baselib::xdr::{ScBytes, ScVal};
    Ok(ScVal::Bytes(ScBytes(value.try_into().map_err(
        |error| Error::InvalidInput(format!("Soroban bytes too large: {error}")),
    )?)))
}

fn stellar_map(
    entries: impl IntoIterator<Item = (&'static str, stellar_baselib::xdr::ScVal)>,
) -> Result<stellar_baselib::xdr::ScVal> {
    use stellar_baselib::xdr::{ScMap, ScMapEntry, ScVal, VecM};
    let mut entries = entries
        .into_iter()
        .map(|(name, val)| {
            Ok(ScMapEntry {
                key: stellar_symbol(name)?,
                val,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    entries.sort_by(|left, right| {
        let ScVal::Symbol(left) = &left.key else {
            unreachable!()
        };
        let ScVal::Symbol(right) = &right.key else {
            unreachable!()
        };
        left.0.as_slice().cmp(right.0.as_slice())
    });
    Ok(ScVal::Map(Some(ScMap(VecM::try_from(entries).map_err(
        |error| Error::InvalidInput(format!("Soroban map too large: {error}")),
    )?))))
}

fn stellar_unit_enum(variant: &str) -> Result<stellar_baselib::xdr::ScVal> {
    use stellar_baselib::xdr::{ScVal, ScVec, VecM};
    Ok(ScVal::Vec(Some(ScVec(
        VecM::try_from(vec![stellar_symbol(variant)?])
            .map_err(|error| Error::InvalidInput(format!("Soroban enum too large: {error}")))?,
    ))))
}

fn stellar_invocation(
    function: &str,
    args: impl IntoIterator<Item = stellar_baselib::xdr::ScVal>,
) -> Result<StellarInvocationV1> {
    Ok(StellarInvocationV1 {
        function: function.into(),
        args_xdr_hex: args
            .into_iter()
            .map(encode_stellar_scval)
            .collect::<Result<Vec<_>>>()?,
    })
}
fn stellar_role_operator<'a>(state: &'a RouteStateV1, role: &str) -> Result<&'a str> {
    state
        .contracts
        .get(&format!("stellar_role:{role}"))
        .map(String::as_str)
        .or_else(|| {
            state
                .effective_config
                .get(&format!("authority:stellar:role:{role}"))
                .and_then(serde_json::Value::as_str)
        })
        .ok_or_else(|| {
            Error::Custody(format!(
                "route has no effective holder for Stellar role {role}"
            ))
        })
}

pub fn stellar_operation_authorizer<'a>(
    state: &'a RouteStateV1,
    operation: &'a OperationV1,
) -> Result<&'a str> {
    match operation {
        OperationV1::SetDefaultFee { .. } | OperationV1::SetDestinationFee { .. } => {
            stellar_role_operator(state, "FEE_CONFIG_MANAGER_ROLE")
        }
        OperationV1::SetInboundRateLimit { .. } | OperationV1::SetOutboundRateLimit { .. } => {
            stellar_role_operator(state, "RATE_LIMITER_MANAGER_ROLE")
        }
        OperationV1::PauseEmergency => stellar_role_operator(state, "PAUSER_ROLE"),
        OperationV1::UnpauseEmergency => stellar_role_operator(state, "UNPAUSER_ROLE"),
        OperationV1::SetStellarUlnConfig { caller, .. }
        | OperationV1::SetStellarExecutorConfig { caller, .. } => Ok(caller),
        OperationV1::SendLeg {
            vm: Vm::Stellar,
            intent,
        } => Ok(&intent.sender),
        OperationV1::ExecuteReceive {
            vm: Vm::Stellar, ..
        } => state
            .contracts
            .get("stellar_recovery_executor")
            .map(String::as_str)
            .or_else(|| {
                state
                    .effective_config
                    .get("authority:stellar:recovery_executor")
                    .and_then(serde_json::Value::as_str)
            })
            .ok_or_else(|| {
                Error::Custody("route has no effective Stellar recovery executor".into())
            }),
        OperationV1::AcceptStellarOwnership => state
            .effective_config
            .get("stellar:pending_owner")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| {
                Error::Custody("pending Stellar owner readback is required for acceptance".into())
            }),
        _ => state
            .contracts
            .get("stellar_owner")
            .map(String::as_str)
            .ok_or_else(|| Error::Custody("route has no recorded stellar_owner".into())),
    }
}

pub fn evm_operation_authorizer<'a>(
    state: &'a RouteStateV1,
    operation: &'a OperationV1,
) -> Result<&'a str> {
    match operation {
        OperationV1::DeployEvmOft { deployer, .. } => Ok(deployer),
        OperationV1::SetEvmUlnConfig { caller, .. }
        | OperationV1::SetEvmExecutorConfig { caller, .. } => Ok(caller),
        OperationV1::SendLeg {
            vm: Vm::Evm,
            intent,
        } => Ok(&intent.sender),
        _ => state
            .contracts
            .get("evm_owner")
            .map(String::as_str)
            .ok_or_else(|| Error::Custody("route has no recorded evm_owner".into())),
    }
}

pub fn build_stellar_operation_for_route(
    state: &RouteStateV1,
    operation: &OperationV1,
) -> Result<StellarInvocationV1> {
    use stellar_baselib::xdr::{Int128Parts, ScVal};

    let owner = state
        .contracts
        .get("stellar_owner")
        .ok_or_else(|| Error::Custody("route has no recorded stellar_owner".into()))?;
    let oapp = state
        .contracts
        .get("stellar_oft")
        .ok_or_else(|| Error::Custody("route has no recorded stellar_oft".into()))?;
    match operation {
        OperationV1::BeginStellarOwnershipTransfer { new_owner, ttl } => stellar_invocation(
            "begin_ownership_transfer",
            [stellar_address(new_owner)?, ScVal::U32(*ttl)],
        ),
        OperationV1::AcceptStellarOwnership => stellar_invocation("accept_ownership", []),
        OperationV1::CancelStellarOwnershipTransfer => {
            let pending = state
                .effective_config
                .get("stellar:pending_owner")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| {
                    Error::Custody(
                        "pending Stellar owner readback is required for cancellation".into(),
                    )
                })?;
            stellar_invocation(
                "begin_ownership_transfer",
                [stellar_address(pending)?, ScVal::U32(0)],
            )
        }
        OperationV1::SetStellarDelegate { delegate } => stellar_invocation(
            "set_delegate",
            [stellar_address(delegate)?, stellar_address(owner)?],
        ),
        OperationV1::SetStellarPeer { remote_eid, peer } => {
            let peer = if peer.starts_with("0x") {
                crate::codec::evm_address_to_bytes32(peer)?
            } else {
                crate::codec::strkey_to_bytes32(peer)?
            };
            stellar_invocation(
                "set_peer",
                [
                    ScVal::U32(*remote_eid),
                    stellar_bytes(peer.to_vec())?,
                    stellar_address(owner)?,
                ],
            )
        }
        OperationV1::SetStellarSendLibrary {
            remote_eid,
            library,
        } => stellar_invocation(
            "set_send_library",
            [
                stellar_address(owner)?,
                stellar_address(oapp)?,
                ScVal::U32(*remote_eid),
                stellar_address(library)?,
            ],
        ),
        OperationV1::SetStellarReceiveLibrary {
            remote_eid,
            library,
            grace_period_seconds,
        } => stellar_invocation(
            "set_receive_library",
            [
                stellar_address(owner)?,
                stellar_address(oapp)?,
                ScVal::U32(*remote_eid),
                stellar_address(library)?,
                ScVal::U64(*grace_period_seconds),
            ],
        ),
        OperationV1::RemoveStellarReceiveLibraryTimeout { remote_eid } => stellar_invocation(
            "set_receive_library_timeout",
            [
                stellar_address(owner)?,
                stellar_address(oapp)?,
                ScVal::U32(*remote_eid),
                ScVal::Void,
            ],
        ),
        OperationV1::SetStellarReceiveOptions {
            remote_eid,
            message_type,
            options,
        } => {
            use stellar_baselib::xdr::{ScVec, VecM};
            let option = stellar_map([
                ("eid", ScVal::U32(*remote_eid)),
                ("msg_type", ScVal::U32(u32::from(*message_type))),
                (
                    "options",
                    stellar_bytes(hex::decode(options.trim_start_matches("0x")).map_err(
                        |error| Error::InvalidInput(format!("invalid enforced options: {error}")),
                    )?)?,
                ),
            ])?;
            stellar_invocation(
                "set_enforced_options",
                [
                    ScVal::Vec(Some(ScVec(VecM::try_from(vec![option]).map_err(
                        |error| Error::InvalidInput(format!("options vector too large: {error}")),
                    )?))),
                    stellar_address(owner)?,
                ],
            )
        }
        OperationV1::SetDefaultFee { bps } => stellar_invocation(
            "set_default_fee_bps",
            [
                ScVal::U32(*bps),
                stellar_address(stellar_role_operator(state, "FEE_CONFIG_MANAGER_ROLE")?)?,
            ],
        ),
        OperationV1::SetDestinationFee { remote_eid, bps } => stellar_invocation(
            "set_fee_bps",
            [
                ScVal::U32(*remote_eid),
                ScVal::U32(*bps),
                stellar_address(stellar_role_operator(state, "FEE_CONFIG_MANAGER_ROLE")?)?,
            ],
        ),
        OperationV1::SetFeeRecipient { recipient } => stellar_invocation(
            "set_fee_deposit_address",
            [stellar_address(recipient)?, stellar_address(owner)?],
        ),
        OperationV1::SetMessageInspector { inspector } => stellar_invocation(
            "set_msg_inspector",
            [
                inspector
                    .as_deref()
                    .map(stellar_address)
                    .transpose()?
                    .unwrap_or(ScVal::Void),
                stellar_address(owner)?,
            ],
        ),
        OperationV1::SetInboundRateLimit {
            remote_eid,
            limit_raw,
            window_seconds,
            mode,
        }
        | OperationV1::SetOutboundRateLimit {
            remote_eid,
            limit_raw,
            window_seconds,
            mode,
        } => {
            let limit = i128::try_from(*limit_raw)
                .map_err(|_| Error::InvalidInput("Stellar rate limit exceeds i128".into()))?;
            let config = stellar_map([
                (
                    "limit",
                    ScVal::I128(stellar_i128_parts(limit)),
                ),
                (
                    "mode",
                    stellar_unit_enum(if mode == "net" {
                        "Net"
                    } else if mode == "gross" {
                        "Gross"
                    } else {
                        return Err(Error::InvalidInput(
                            "rate-limit mode must be net or gross".into(),
                        ));
                    })?,
                ),
                ("window_seconds", ScVal::U64(*window_seconds)),
            ])?;
            stellar_invocation(
                "set_rate_limit",
                [
                    stellar_unit_enum(
                        if matches!(operation, OperationV1::SetInboundRateLimit { .. }) {
                            "Inbound"
                        } else {
                            "Outbound"
                        },
                    )?,
                    ScVal::U32(*remote_eid),
                    config,
                    stellar_address(stellar_role_operator(state, "RATE_LIMITER_MANAGER_ROLE")?)?,
                ],
            )
        }
        OperationV1::PauseEmergency => stellar_invocation(
            "pause",
            [stellar_address(stellar_role_operator(
                state,
                "PAUSER_ROLE",
            )?)?],
        ),
        OperationV1::UnpauseEmergency => stellar_invocation(
            "unpause",
            [stellar_address(stellar_role_operator(
                state,
                "UNPAUSER_ROLE",
            )?)?],
        ),
        OperationV1::SetTtlConfig {
            instance_threshold,
            instance_extend_to,
            persistent_threshold,
            persistent_extend_to,
        } => stellar_invocation(
            "set_ttl_configs",
            [
                stellar_map([
                    ("extend_to", ScVal::U32(*instance_extend_to)),
                    ("threshold", ScVal::U32(*instance_threshold)),
                ])?,
                stellar_map([
                    ("extend_to", ScVal::U32(*persistent_extend_to)),
                    ("threshold", ScVal::U32(*persistent_threshold)),
                ])?,
            ],
        ),
        OperationV1::FreezeTtlConfig { .. } => stellar_invocation("freeze_ttl_configs", []),
        OperationV1::ExtendInstanceTtl { ledgers } => stellar_invocation(
            "extend_instance_ttl",
            [ScVal::U32(*ledgers), ScVal::U32(*ledgers)],
        ),
        OperationV1::GrantRole { role, address } => stellar_invocation(
            "grant_role",
            [
                stellar_address(address)?,
                stellar_symbol(role)?,
                stellar_address(owner)?,
            ],
        ),
        OperationV1::RevokeRole { role, address } => stellar_invocation(
            "revoke_role",
            [
                stellar_address(address)?,
                stellar_symbol(role)?,
                stellar_address(owner)?,
            ],
        ),
        OperationV1::SetRoleAdmin { role, admin_role } => stellar_invocation(
            "set_role_admin",
            [stellar_symbol(role)?, stellar_symbol(admin_role)?],
        ),
        OperationV1::RemoveRoleAdmin { role, .. } => {
            stellar_invocation("remove_role_admin", [stellar_symbol(role)?])
        }
        OperationV1::ContainOutbound { snapshot } => {
            let mutation = containment_mutation(state, snapshot, false)?;
            build_stellar_operation_for_route(state, &mutation)
        }
        OperationV1::RestoreOutbound { snapshot } => {
            let mutation = containment_mutation(state, snapshot, true)?;
            build_stellar_operation_for_route(state, &mutation)
        }
        OperationV1::CommitVerification {
            vm: Vm::Stellar,
            message,
        } => {
            qualify_message_for_route(state, message)?;
            stellar_invocation(
                "commit_verification",
                [
                    stellar_bytes(
                        hex::decode(message.packet_header.trim_start_matches("0x")).map_err(
                            |error| Error::InvalidInput(format!("invalid packet header: {error}")),
                        )?,
                    )?,
                    stellar_bytes(
                        hex::decode(message.payload_keccak256.trim_start_matches("0x")).map_err(
                            |error| Error::InvalidInput(format!("invalid payload hash: {error}")),
                        )?,
                    )?,
                ],
            )
        }
        OperationV1::ExecuteReceive {
            vm: Vm::Stellar,
            message,
        } => {
            qualify_message_for_route(state, message)?;
            let mut invocation = build_stellar_operation(operation)?;
            invocation.args_xdr_hex.insert(
                0,
                encode_stellar_scval(stellar_address(stellar_operation_authorizer(
                    state, operation,
                )?)?)?,
            );
            invocation
                .args_xdr_hex
                .push(encode_stellar_scval(ScVal::I128(Int128Parts {
                    hi: 0,
                    lo: 0,
                }))?);
            Ok(invocation)
        }
        _ => build_stellar_operation(operation),
    }
}

pub fn build_stellar_operation(operation: &OperationV1) -> Result<StellarInvocationV1> {
    use stellar_baselib::xdr::{Limits, ScBytes, ScVal, WriteXdr as _};

    let encode = |value: ScVal| {
        value
            .to_xdr(Limits::none())
            .map(hex::encode)
            .map_err(|error| Error::InvalidInput(format!("Stellar argument XDR failed: {error}")))
    };
    match operation {
        OperationV1::SetStellarUlnConfig {
            remote_eid,
            caller,
            oapp,
            library,
            config_sha256,
            config,
            direction,
        } => {
            let typed: UlnConfigType3V1 = serde_json::from_value(config.clone())?;
            typed.validate()?;
            if &typed.config_sha256()? != config_sha256 {
                return Err(Error::Custody("Stellar ULN config digest mismatch".into()));
            }
            stellar_set_config_invocation(
                caller,
                oapp,
                library,
                *remote_eid,
                match direction.as_str() {
                    "send" => 2,
                    "receive" => 3,
                    other => {
                        return Err(Error::InvalidInput(format!(
                            "Stellar ULN direction must be send or receive, got {other}"
                        )))
                    }
                },
                crate::codec::encode_stellar_oapp_uln_config(
                    &crate::codec::StellarOAppUlnConfig {
                        use_default_required_dvns: typed.use_default_required_dvns,
                        use_default_confirmations: typed.use_default_confirmations,
                        use_default_optional_dvns: typed.use_default_optional_dvns,
                        confirmations: typed.confirmations.into(),
                        required_dvns: typed.required_dvns,
                        optional_dvns: typed.optional_dvns,
                        optional_dvn_threshold: typed.optional_threshold.into(),
                    },
                )?,
            )
        }
        OperationV1::SetStellarExecutorConfig {
            remote_eid,
            caller,
            oapp,
            library,
            config_sha256,
            config,
        } => {
            let typed: ExecutorConfigType3V1 = serde_json::from_value(config.clone())?;
            typed.validate()?;
            if &typed.config_sha256()? != config_sha256 {
                return Err(Error::Custody(
                    "Stellar executor config digest mismatch".into(),
                ));
            }
            stellar_set_config_invocation(
                caller,
                oapp,
                library,
                *remote_eid,
                1,
                crate::codec::encode_stellar_executor_config(
                    typed.max_message_size,
                    &typed.executor,
                )?,
            )
        }
        OperationV1::SendLeg {
            vm: Vm::Stellar,
            intent,
        } => {
            use std::str::FromStr as _;
            use stellar_baselib::xdr::{
                AccountId, Int128Parts, PublicKey, ScAddress, ScMap, ScMapEntry, ScSymbol, StringM,
                Uint256, VecM,
            };
            use stellar_strkey::Strkey;

            if intent.direction != Direction::StellarToEvm {
                return Err(Error::Conflict(
                    "Stellar send operation carries a non-Stellar source direction".into(),
                ));
            }
            let symbol = |value: &str| {
                Ok::<_, Error>(ScVal::Symbol(ScSymbol(
                    StringM::try_from(value.as_bytes().to_vec()).map_err(|error| {
                        Error::InvalidInput(format!("invalid Soroban symbol: {error}"))
                    })?,
                )))
            };
            let map = |entries: Vec<(ScVal, ScVal)>| {
                Ok::<_, Error>(ScVal::Map(Some(ScMap(
                    VecM::try_from(
                        entries
                            .into_iter()
                            .map(|(key, val)| ScMapEntry { key, val })
                            .collect::<Vec<_>>(),
                    )
                    .map_err(|error| {
                        Error::InvalidInput(format!("Soroban map too large: {error}"))
                    })?,
                ))))
            };
            let i128_val = |value: &str| {
                let value: i128 = value
                    .parse()
                    .map_err(|_| Error::InvalidInput("Stellar send integer exceeds i128".into()))?;
                Ok::<_, Error>(ScVal::I128(stellar_i128_parts(value)))
            };
            let bytes = |value: Vec<u8>| {
                Ok::<_, Error>(ScVal::Bytes(ScBytes(value.try_into().map_err(
                    |error| Error::InvalidInput(format!("Soroban bytes too large: {error}")),
                )?)))
            };
            let address = |value: &str| match Strkey::from_str(value) {
                Ok(Strkey::PublicKeyEd25519(key)) => Ok(ScVal::Address(ScAddress::Account(
                    AccountId(PublicKey::PublicKeyTypeEd25519(Uint256(key.0))),
                ))),
                _ => Err(Error::InvalidInput(format!(
                    "Stellar send address must be a classic G... account: {value}"
                ))),
            };
            let send_param = map(vec![
                (symbol("amount_ld")?, i128_val(&intent.amount_raw)?),
                (symbol("compose_msg")?, bytes(Vec::new())?),
                (symbol("dst_eid")?, ScVal::U32(intent.destination_eid)),
                (
                    symbol("extra_options")?,
                    bytes(
                        hex::decode(intent.extra_options.trim_start_matches("0x")).map_err(
                            |error| Error::InvalidInput(format!("invalid extra options: {error}")),
                        )?,
                    )?,
                ),
                (
                    symbol("min_amount_ld")?,
                    i128_val(&intent.minimum_received_raw)?,
                ),
                (symbol("oft_cmd")?, bytes(Vec::new())?),
                (
                    symbol("to")?,
                    bytes(crate::codec::evm_address_to_bytes32(&intent.to)?.to_vec())?,
                ),
            ])?;
            let fee = map(vec![
                (
                    symbol("zro_fee")?,
                    ScVal::I128(Int128Parts { hi: 0, lo: 0 }),
                ),
                (symbol("native_fee")?, i128_val(&intent.native_fee_raw)?),
            ])?;
            Ok(StellarInvocationV1 {
                function: "send".into(),
                args_xdr_hex: vec![
                    encode(address(&intent.sender)?)?,
                    encode(send_param)?,
                    encode(fee)?,
                    encode(address(&intent.refund_address)?)?,
                ],
            })
        }
        OperationV1::ExecuteReceive {
            vm: Vm::Stellar,
            message,
        } => {
            use stellar_baselib::xdr::{ScMap, ScMapEntry, ScSymbol, StringM, VecM};

            let symbol = |value: &str| {
                Ok::<_, Error>(ScVal::Symbol(ScSymbol(
                    StringM::try_from(value.as_bytes().to_vec()).map_err(|error| {
                        Error::InvalidInput(format!("invalid Soroban symbol: {error}"))
                    })?,
                )))
            };
            let bytes = |value: Vec<u8>| {
                Ok::<_, Error>(ScVal::Bytes(ScBytes(value.try_into().map_err(
                    |error| Error::InvalidInput(format!("Soroban bytes too large: {error}")),
                )?)))
            };
            let header = decode_packet_header(&message.packet_header)?;
            let origin = ScVal::Map(Some(ScMap(
                VecM::try_from(vec![
                    ScMapEntry {
                        key: symbol("nonce")?,
                        val: ScVal::U64(header.nonce),
                    },
                    ScMapEntry {
                        key: symbol("sender")?,
                        val: bytes(header.sender.to_vec())?,
                    },
                    ScMapEntry {
                        key: symbol("src_eid")?,
                        val: ScVal::U32(header.source_eid),
                    },
                ])
                .map_err(|error| Error::InvalidInput(format!("origin map too large: {error}")))?,
            )));
            Ok(StellarInvocationV1 {
                function: "lz_receive".into(),
                args_xdr_hex: vec![
                    encode(origin)?,
                    encode(bytes(
                        hex::decode(message.guid.trim_start_matches("0x")).map_err(|error| {
                            Error::InvalidInput(format!("invalid guid hex: {error}"))
                        })?,
                    )?)?,
                    encode(bytes(
                        hex::decode(message.message.trim_start_matches("0x")).map_err(|error| {
                            Error::InvalidInput(format!("invalid message hex: {error}"))
                        })?,
                    )?)?,
                    encode(bytes(Vec::new())?)?,
                ],
            })
        }
        _ => Err(Error::InvalidInput(format!(
            "stellar operation {} lacks a complete native argument representation",
            crate::stellar::operation_label(operation)
        ))),
    }
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RouteField {
    /// Free-form route config key.
    Config(String),
    /// Peer contract for a remote endpoint id.
    Peer(u32),
}

/// Typed drift entry.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RouteDriftV1 {
    pub field: RouteField,
    pub desired: String,
    pub effective: String,
}

/// Result of comparing a desired route against recorded effective state.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RouteComparisonV1 {
    pub route_id: String,
    pub drift: Vec<RouteDriftV1>,
    pub converged: bool,
}

fn config_string(config: &BTreeMap<String, serde_json::Value>, key: &str) -> String {
    config
        .get(key)
        .and_then(|value| value.as_str())
        .unwrap_or_default()
        .to_string()
}

/// Compares the desired route against the recorded effective route state.
/// Every config key and peer contract present on either side must agree;
/// mismatches and missing sides are drift.
pub fn compare_routes(desired: &DesiredRouteV1, state: &RouteStateV1) -> Result<RouteComparisonV1> {
    if state.route_id != desired.route_id {
        return Err(Error::Conflict(format!(
            "route state {} does not match desired route {}",
            state.route_id, desired.route_id
        )));
    }
    let mut keys: BTreeSet<&String> = desired.config.keys().collect();
    keys.extend(state.effective_config.keys());
    let mut drift = Vec::new();
    for key in keys {
        let desired_value = config_string(&desired.config, key);
        let effective_value = config_string(&state.effective_config, key);
        if desired_value != effective_value {
            drift.push(RouteDriftV1 {
                field: RouteField::Config(key.clone()),
                desired: desired_value,
                effective: effective_value,
            });
        }
    }
    let peers = [
        (
            desired.identity.stellar_eid,
            desired.identity.stellar_endpoint.as_str(),
        ),
        (
            desired.identity.evm_eid,
            desired.identity.evm_endpoint.as_str(),
        ),
    ];
    for (eid, desired_peer) in peers {
        let effective_peer = state
            .contracts
            .get(format!("peer:{eid}").as_str())
            .map(String::as_str)
            .unwrap_or_default();
        if effective_peer != desired_peer {
            drift.push(RouteDriftV1 {
                field: RouteField::Peer(eid),
                desired: desired_peer.to_string(),
                effective: effective_peer.to_string(),
            });
        }
    }
    Ok(RouteComparisonV1 {
        route_id: desired.route_id.clone(),
        converged: drift.is_empty(),
        drift,
    })
}

/// Checked native adapter trait for route comparison.
pub trait RouteComparisonAdapter {
    fn compare(&self, desired: &DesiredRouteV1, state: &RouteStateV1) -> Result<RouteComparisonV1>;
}
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct UlnConfigType3V1 {
    pub required_dvns: Vec<String>,
    pub optional_dvns: Vec<String>,
    pub optional_threshold: u8,
    pub confirmations: u32,
    #[serde(default)]
    pub use_default_confirmations: bool,
    #[serde(default)]
    pub use_default_required_dvns: bool,
    #[serde(default)]
    pub use_default_optional_dvns: bool,
}

impl UlnConfigType3V1 {
    /// Checks structural invariants of the security config.
    pub fn validate(&self) -> Result<()> {
        let check = |dvns: &[String]| {
            if dvns.iter().any(|dvn| dvn.trim().is_empty()) {
                return Err(Error::InvalidInput("dvn address must not be empty".into()));
            }
            if dvns.iter().collect::<BTreeSet<_>>().len() != dvns.len() {
                return Err(Error::InvalidInput("duplicate dvn address".into()));
            }
            Ok(())
        };
        check(&self.required_dvns)?;
        check(&self.optional_dvns)?;
        if self
            .required_dvns
            .iter()
            .any(|dvn| self.optional_dvns.contains(dvn))
        {
            return Err(Error::InvalidInput(
                "a dvn cannot be both required and optional".into(),
            ));
        }
        if usize::from(self.optional_threshold) > self.optional_dvns.len() {
            return Err(Error::InvalidInput(
                "optional threshold exceeds optional dvn count".into(),
            ));
        }
        if self.confirmations == 0 {
            return Err(Error::InvalidInput(
                "confirmations must be at least 1".into(),
            ));
        }
        Ok(())
    }

    /// Deterministic canonical hash binding the config into operations.
    pub fn config_sha256(&self) -> Result<String> {
        self.validate()?;
        crate::canonical_sha256(self)
    }
}

/// Type-3 executor config.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct ExecutorConfigType3V1 {
    pub max_message_size: u32,
    /// Explicit executor address; empty means the default executor.
    pub executor: String,
}

impl ExecutorConfigType3V1 {
    /// Checks structural invariants of the executor config.
    pub fn validate(&self) -> Result<()> {
        if self.max_message_size == 0 || self.executor.trim().is_empty() {
            return Err(Error::InvalidInput(
                "executor config requires a nonzero max_message_size and executor".into(),
            ));
        }
        Ok(())
    }

    /// Deterministic canonical hash binding the config into operations.
    pub fn config_sha256(&self) -> Result<String> {
        self.validate()?;
        crate::canonical_sha256(self)
    }
}

/// Builds the typed ULN operation for a config.
pub fn set_uln_operation(
    vm: Vm,
    remote_eid: u32,
    direction: &str,
    caller: &str,
    oapp: &str,
    library: &str,
    config: &UlnConfigType3V1,
) -> Result<OperationV1> {
    let config_sha256 = config.config_sha256()?;
    let config = serde_json::to_value(config)?;
    Ok(match vm {
        Vm::Stellar => OperationV1::SetStellarUlnConfig {
            direction: direction.into(),
            caller: caller.into(),
            oapp: oapp.into(),
            library: library.into(),
            remote_eid,
            config_sha256,
            config,
        },
        Vm::Evm => OperationV1::SetEvmUlnConfig {
            remote_eid,
            direction: direction.into(),
            caller: caller.into(),
            oapp: oapp.into(),
            library: library.into(),
            config_sha256,
            config,
        },
    })
}

/// Builds the typed executor operation for a config.
pub fn set_executor_operation(
    vm: Vm,
    remote_eid: u32,
    caller: &str,
    oapp: &str,
    library: &str,
    config: &ExecutorConfigType3V1,
) -> Result<OperationV1> {
    let config_sha256 = config.config_sha256()?;
    let config = serde_json::to_value(config)?;
    Ok(match vm {
        Vm::Stellar => OperationV1::SetStellarExecutorConfig {
            remote_eid,
            caller: caller.into(),
            oapp: oapp.into(),
            library: library.into(),
            config_sha256,
            config,
        },
        Vm::Evm => OperationV1::SetEvmExecutorConfig {
            remote_eid,
            caller: caller.into(),
            oapp: oapp.into(),
            library: library.into(),
            config_sha256,
            config,
        },
    })
}

pub fn containment_snapshot(
    state: &RouteStateV1,
    direction: Direction,
) -> Result<crate::domain::ContainmentSnapshotV1> {
    let (vm, remote_eid) = match direction {
        Direction::StellarToEvm => (Vm::Stellar, state.identity.evm_eid),
        Direction::EvmToStellar => (Vm::Evm, state.identity.stellar_eid),
    };
    match direction {
        Direction::StellarToEvm => {
            stellar_role_operator(state, "RATE_LIMITER_MANAGER_ROLE")?;
        }
        Direction::EvmToStellar => {
            state
                .contracts
                .get("evm_owner")
                .filter(|owner| !owner.is_empty())
                .ok_or_else(|| {
                    Error::Custody("containment requires the effective EVM owner".into())
                })?;
        }
    }
    let peer_key = format!("peer:{remote_eid}");
    let peer = state
        .contracts
        .get(&peer_key)
        .cloned()
        .ok_or_else(|| Error::Custody(format!("containment requires effective {peer_key}")))?;
    let receive_key = crate::route::config_key_receive_library(vm, remote_eid);
    let receive_library = state
        .effective_config
        .get(&receive_key)
        .and_then(serde_json::Value::as_str)
        .map(String::from)
        .ok_or_else(|| Error::Custody(format!("containment requires effective {receive_key}")))?;
    let required_receive_keys = [
        receive_key,
        crate::route::config_key_uln_config(vm, remote_eid, "receive")?,
        crate::route::config_key_executor_config(vm, remote_eid),
    ];
    let options_prefix = format!(
        "receive_options:{}:{remote_eid}:",
        crate::route::vm_label(vm)
    );
    let receive_config = state
        .effective_config
        .iter()
        .filter(|(key, _)| required_receive_keys.contains(key) || key.starts_with(&options_prefix))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect::<BTreeMap<_, _>>();
    if required_receive_keys
        .iter()
        .any(|key| !receive_config.contains_key(key))
        || !receive_config
            .keys()
            .any(|key| key.starts_with(&options_prefix))
    {
        return Err(Error::Custody(
            "containment requires complete effective receive library/ULN/executor/options state"
                .into(),
        ));
    }
    let restore_operation = match direction {
        Direction::StellarToEvm => {
            let read_string = |suffix: &str| {
                let key = format!("outbound_rate_limit:stellar:{remote_eid}:{suffix}");
                state
                    .effective_config
                    .get(&key)
                    .and_then(serde_json::Value::as_str)
                    .map(String::from)
                    .ok_or_else(|| Error::Custody(format!("containment requires effective {key}")))
            };
            OperationV1::SetOutboundRateLimit {
                remote_eid,
                limit_raw: read_string("limit_raw")?
                    .parse()
                    .map_err(|_| Error::Custody("effective outbound limit is not u128".into()))?,
                window_seconds: read_string("window_seconds")?
                    .parse()
                    .map_err(|_| Error::Custody("effective outbound window is not u64".into()))?,
                mode: read_string("mode")?,
            }
        }
        Direction::EvmToStellar => {
            let key = crate::route::config_key_send_library(vm, remote_eid);
            OperationV1::SetEvmSendLibrary {
                remote_eid,
                library: state
                    .effective_config
                    .get(&key)
                    .and_then(serde_json::Value::as_str)
                    .map(String::from)
                    .ok_or_else(|| {
                        Error::Custody(format!("containment requires effective {key}"))
                    })?,
            }
        }
    };
    Ok(crate::domain::ContainmentSnapshotV1 {
        schema_name: "containment_snapshot".into(),
        schema_version: crate::domain::SCHEMA_VERSION,
        direction,
        remote_eid,
        restore_operation: Box::new(restore_operation),
        peer,
        receive_library,
        receive_config_sha256: crate::canonical_sha256(&receive_config)?,
    })
}

pub fn containment_mutation(
    state: &RouteStateV1,
    snapshot: &crate::domain::ContainmentSnapshotV1,
    restore: bool,
) -> Result<OperationV1> {
    if snapshot.schema_name != "containment_snapshot"
        || snapshot.schema_version != crate::domain::SCHEMA_VERSION
    {
        return Err(Error::InvalidInput(
            "unsupported containment snapshot schema".into(),
        ));
    }
    let current = containment_snapshot(state, snapshot.direction)?;
    if current.peer != snapshot.peer
        || current.receive_library != snapshot.receive_library
        || current.receive_config_sha256 != snapshot.receive_config_sha256
    {
        return Err(Error::Conflict(
            "peer or inbound receive configuration changed since containment snapshot".into(),
        ));
    }
    let valid_restore = matches!(
        (snapshot.direction, snapshot.restore_operation.as_ref()),
        (
            Direction::StellarToEvm,
            OperationV1::SetOutboundRateLimit { remote_eid, .. }
        ) if *remote_eid == snapshot.remote_eid
    ) || matches!(
        (snapshot.direction, snapshot.restore_operation.as_ref()),
        (
            Direction::EvmToStellar,
            OperationV1::SetEvmSendLibrary { remote_eid, .. }
        ) if *remote_eid == snapshot.remote_eid
    );
    if !valid_restore {
        return Err(Error::Custody(
            "containment snapshot carries an invalid restore operation".into(),
        ));
    }
    if restore {
        return Ok((*snapshot.restore_operation).clone());
    }
    Ok(match snapshot.direction {
        Direction::StellarToEvm => {
            let OperationV1::SetOutboundRateLimit {
                remote_eid,
                window_seconds,
                mode,
                ..
            } = snapshot.restore_operation.as_ref()
            else {
                return Err(Error::Custody(
                    "invalid Stellar containment restore operation".into(),
                ));
            };
            OperationV1::SetOutboundRateLimit {
                remote_eid: *remote_eid,
                limit_raw: 0,
                window_seconds: *window_seconds,
                mode: mode.clone(),
            }
        }
        Direction::EvmToStellar => OperationV1::SetEvmSendLibrary {
            remote_eid: snapshot.remote_eid,
            library: state
                .effective_config
                .get("endpoint:blocked_library:evm")
                .and_then(serde_json::Value::as_str)
                .map(String::from)
                .ok_or_else(|| {
                    Error::Custody(
                        "recorded EVM EndpointV2 blockedLibrary is required for containment".into(),
                    )
                })?,
        },
    })
}

/// Reports recorded containment state without mutating either chain.
pub fn containment_status(state: &std::path::Path) -> Result<crate::output::CommandData> {
    let state = crate::state::RouteStore::open(state)?.load_state()?;
    Ok(crate::output::CommandData {
        result: serde_json::json!({
            "stellar": state.effective_config.get("containment:stellar"),
            "evm": state.effective_config.get("containment:evm")
        }),
        artifact: None,
    })
}

fn calldata_word_u32(value: u32) -> Vec<u8> {
    let mut word = vec![0u8; 32];
    word[28..32].copy_from_slice(&value.to_be_bytes());
    word
}

fn calldata_word_u64(value: u64) -> Vec<u8> {
    let mut word = vec![0u8; 32];
    word[24..32].copy_from_slice(&value.to_be_bytes());
    word
}

fn calldata_word_u16(value: u16) -> Vec<u8> {
    let mut word = vec![0u8; 32];
    word[30..32].copy_from_slice(&value.to_be_bytes());
    word
}

fn calldata_selector(signature: &str) -> Vec<u8> {
    crate::evm::keccak256_of(signature.as_bytes())[..4].to_vec()
}

fn calldata_word_address(address: &str) -> Result<Vec<u8>> {
    let parsed = crate::evm::parse_address(address)?;
    let mut word = vec![0u8; 12];
    word.extend_from_slice(parsed.as_slice());
    Ok(word)
}

fn calldata_word_peer(peer: &str) -> Result<Vec<u8>> {
    let hex_body = peer
        .strip_prefix("0x")
        .ok_or_else(|| Error::InvalidInput("peer must be 0x-prefixed".into()))?;
    // A peer is the full LayerZero bytes32: either a raw 32-byte value
    // (Stellar contract) or a 20-byte EVM address left-padded.
    match hex_body.len() {
        64 => hex::decode(hex_body)
            .map_err(|error| Error::InvalidInput(format!("invalid peer hex: {error}"))),
        40 => Ok(calldata_word_address(peer)?),
        other => Err(Error::InvalidInput(format!(
            "peer must be 40 or 64 hex characters, got {other}"
        ))),
    }
}
fn calldata_bytes_element(data: &[u8]) -> Vec<u8> {
    let mut element = vec![0u8; 24];
    element.extend_from_slice(&(data.len() as u64).to_be_bytes());
    element.extend_from_slice(data);
    let padding = (32 - data.len() % 32) % 32;
    element.extend(std::iter::repeat_n(0u8, padding));
    element
}

pub fn encode_calldata_for_route(state: &RouteStateV1, operation: &OperationV1) -> Result<Vec<u8>> {
    let oapp = state
        .contracts
        .get("evm_oft")
        .ok_or_else(|| Error::Custody("route has no recorded evm_oft contract".into()))?;
    match operation {
        OperationV1::SetEvmSendLibrary {
            remote_eid,
            library,
        } => {
            let mut calldata = calldata_selector("setSendLibrary(address,uint32,address)");
            calldata.extend(calldata_word_address(oapp)?);
            calldata.extend(calldata_word_u32(*remote_eid));
            calldata.extend(calldata_word_address(library)?);
            Ok(calldata)
        }
        OperationV1::SetEvmReceiveLibrary {
            remote_eid,
            library,
            grace_period_seconds,
        } => {
            let mut calldata =
                calldata_selector("setReceiveLibrary(address,uint32,address,uint256)");
            calldata.extend(calldata_word_address(oapp)?);
            calldata.extend(calldata_word_u32(*remote_eid));
            calldata.extend(calldata_word_address(library)?);
            calldata.extend(calldata_word_u64(*grace_period_seconds));
            Ok(calldata)
        }
        OperationV1::RemoveEvmReceiveLibraryTimeout { remote_eid } => {
            let mut calldata =
                calldata_selector("setReceiveLibraryTimeout(address,uint32,address,uint256)");
            calldata.extend(calldata_word_address(oapp)?);
            calldata.extend(calldata_word_u32(*remote_eid));
            calldata.extend([0u8; 32]);
            calldata.extend([0u8; 32]);
            Ok(calldata)
        }
        OperationV1::ContainOutbound { snapshot } => {
            let mutation = containment_mutation(state, snapshot, false)?;
            encode_calldata_for_route(state, &mutation)
        }
        OperationV1::RestoreOutbound { snapshot } => {
            let mutation = containment_mutation(state, snapshot, true)?;
            encode_calldata_for_route(state, &mutation)
        }
        OperationV1::CommitVerification {
            vm: Vm::Evm,
            message,
        }
        | OperationV1::ExecuteReceive {
            vm: Vm::Evm,
            message,
        } => {
            qualify_message_for_route(state, message)?;
            encode_calldata(operation)
        }
        _ => encode_calldata(operation),
    }
}

/// Encodes the typed EVM calldata for an operation. Selectors are computed
/// at runtime from canonical signatures. Config-hash operations and
/// deployment/restore operations have no honest single-call encoding in v1
/// and fail closed.
pub fn encode_calldata(operation: &OperationV1) -> Result<Vec<u8>> {
    match operation {
        OperationV1::SetEvmPeer { remote_eid, peer } => {
            let mut calldata = calldata_selector("setPeer(uint32,bytes32)");
            calldata.extend(calldata_word_u32(*remote_eid));
            calldata.extend(calldata_word_peer(peer)?);
            Ok(calldata)
        }
        OperationV1::SetEvmSendLibrary {
            remote_eid,
            library,
        } => {
            let mut calldata = calldata_selector("setSendLibrary(uint32,address)");
            calldata.extend(calldata_word_u32(*remote_eid));
            calldata.extend(calldata_word_address(library)?);
            Ok(calldata)
        }
        OperationV1::SetEvmReceiveLibrary {
            remote_eid,
            library,
            grace_period_seconds,
        } => {
            let mut calldata = calldata_selector("setReceiveLibrary(uint32,address,uint256)");
            calldata.extend(calldata_word_u32(*remote_eid));
            calldata.extend(calldata_word_address(library)?);
            calldata.extend(calldata_word_u64(*grace_period_seconds));
            Ok(calldata)
        }
        OperationV1::SetEvmReceiveOptions {
            remote_eid,
            message_type,
            options,
        } => {
            let options_hex = options.strip_prefix("0x").unwrap_or(options).trim();
            if options_hex.is_empty() {
                return Err(Error::InvalidInput(
                    "enforced options must not be empty".into(),
                ));
            }
            let decoded = hex::decode(options_hex)
                .map_err(|error| Error::InvalidInput(format!("invalid options hex: {error}")))?;
            if decoded.len() < 2 {
                return Err(Error::InvalidInput(
                    "enforced options must carry at least a worker id and size".into(),
                ));
            }
            // Official IOAppOptionsType3:
            // setEnforcedOptions(EnforcedOptionParam[]) with
            // EnforcedOptionParam = (uint32 eid, uint16 msgType, bytes options).
            let mut calldata = calldata_selector("setEnforcedOptions((uint32,uint16,bytes)[])");
            // Head: single dynamic argument offset.
            calldata.extend(calldata_word_u64(32));
            // Array body: length 1, then the element offset relative to the
            // array body start.
            calldata.extend(calldata_word_u64(1));
            calldata.extend(calldata_word_u64(32));
            // Tuple body: eid, msgType, offset to bytes relative to tuple
            // start (three head words), then the bytes payload.
            calldata.extend(calldata_word_u32(*remote_eid));
            calldata.extend(calldata_word_u16(*message_type));
            calldata.extend(calldata_word_u64(3 * 32));
            calldata.extend(calldata_bytes_element(&decoded));
            Ok(calldata)
        }
        OperationV1::TransferEvmOwnership { new_owner } => {
            let mut calldata = calldata_selector("transferOwnership(address)");
            calldata.extend(calldata_word_address(new_owner)?);
            Ok(calldata)
        }
        OperationV1::SetEvmDelegate { delegate } => {
            let mut calldata = calldata_selector("setDelegate(address)");
            calldata.extend(calldata_word_address(delegate)?);
            Ok(calldata)
        }
        OperationV1::SetEvmUlnConfig {
            remote_eid,
            oapp,
            library,
            config_sha256,
            config,
            direction,
            ..
        } => {
            use alloy::sol_types::SolCall as _;
            let typed: UlnConfigType3V1 = serde_json::from_value(config.clone())?;
            typed.validate()?;
            if &typed.config_sha256()? != config_sha256 {
                return Err(Error::Custody("EVM ULN config digest mismatch".into()));
            }
            let encoded = crate::codec::encode_evm_uln_config(
                typed.confirmations.into(),
                &typed.required_dvns,
                &typed.optional_dvns,
                typed.optional_threshold,
            )?;
            Ok(IEndpointConfigV1::setConfigCall {
                oapp: crate::evm::parse_address(oapp)?,
                lib: crate::evm::parse_address(library)?,
                params: vec![SetConfigParamV1 {
                    eid: *remote_eid,
                    configType: match direction.as_str() {
                        "send" => 2,
                        "receive" => 3,
                        other => {
                            return Err(Error::InvalidInput(format!(
                                "EVM ULN direction must be send or receive, got {other}"
                            )))
                        }
                    },
                    config: encoded.into(),
                }],
            }
            .abi_encode())
        }
        OperationV1::SetEvmExecutorConfig {
            remote_eid,
            oapp,
            library,
            config_sha256,
            config,
            ..
        } => {
            use alloy::sol_types::SolCall as _;
            let typed: ExecutorConfigType3V1 = serde_json::from_value(config.clone())?;
            typed.validate()?;
            if &typed.config_sha256()? != config_sha256 {
                return Err(Error::Custody("EVM executor config digest mismatch".into()));
            }
            let encoded =
                crate::codec::encode_evm_executor_config(typed.max_message_size, &typed.executor)?;
            Ok(IEndpointConfigV1::setConfigCall {
                oapp: crate::evm::parse_address(oapp)?,
                lib: crate::evm::parse_address(library)?,
                params: vec![SetConfigParamV1 {
                    eid: *remote_eid,
                    configType: 1,
                    config: encoded.into(),
                }],
            }
            .abi_encode())
        }
        OperationV1::SendLeg {
            vm: Vm::Evm,
            intent,
        } => {
            use alloy::sol_types::SolCall as _;
            use std::str::FromStr as _;

            if intent.direction != Direction::EvmToStellar {
                return Err(Error::Conflict(
                    "EVM send operation carries a non-EVM source direction".into(),
                ));
            }
            let to = crate::codec::strkey_to_bytes32(&intent.to)?;
            let extra_options = hex::decode(intent.extra_options.trim_start_matches("0x"))
                .map_err(|error| Error::InvalidInput(format!("invalid extra options: {error}")))?;
            Ok(IOftSendV1::sendCall {
                sendParam: OftSendParamV1 {
                    dstEid: intent.destination_eid,
                    to: alloy::primitives::FixedBytes(to),
                    amountLD: alloy::primitives::U256::from_str(&intent.amount_raw).map_err(
                        |error| Error::InvalidInput(format!("invalid send amount: {error}")),
                    )?,
                    minAmountLD: alloy::primitives::U256::from_str(&intent.minimum_received_raw)
                        .map_err(|error| {
                            Error::InvalidInput(format!("invalid minimum amount: {error}"))
                        })?,
                    extraOptions: extra_options.into(),
                    composeMsg: Default::default(),
                    oftCmd: Default::default(),
                },
                fee: MessagingFeeV1 {
                    nativeFee: alloy::primitives::U256::from_str(&intent.native_fee_raw).map_err(
                        |error| Error::InvalidInput(format!("invalid native fee: {error}")),
                    )?,
                    lzTokenFee: alloy::primitives::U256::ZERO,
                },
                refundAddress: crate::evm::parse_address(&intent.refund_address)?,
            }
            .abi_encode())
        }
        OperationV1::CommitVerification {
            vm: Vm::Evm,
            message,
        } => {
            let header = hex::decode(message.packet_header.trim_start_matches("0x"))
                .map_err(|error| Error::InvalidInput(format!("invalid packet header: {error}")))?;
            let payload_hash: [u8; 32] =
                hex::decode(message.payload_keccak256.trim_start_matches("0x"))
                    .map_err(|error| Error::InvalidInput(format!("invalid payload hash: {error}")))?
                    .try_into()
                    .map_err(|_| Error::InvalidInput("payload hash must be 32 bytes".into()))?;
            let mut calldata = calldata_selector("commitVerification(bytes,bytes32)");
            calldata.extend(calldata_word_u64(64));
            calldata.extend(payload_hash);
            calldata.extend(calldata_bytes_element(&header));
            Ok(calldata)
        }
        OperationV1::ExecuteReceive {
            vm: Vm::Evm,
            message,
        } => {
            use alloy::sol_types::SolCall as _;
            let bytes32 = |value: &str, field: &str| {
                let bytes = hex::decode(value.trim_start_matches("0x")).map_err(|error| {
                    Error::InvalidInput(format!("invalid {field} hex: {error}"))
                })?;
                let bytes: [u8; 32] = bytes
                    .try_into()
                    .map_err(|_| Error::InvalidInput(format!("{field} must be 32 bytes")))?;
                Ok::<_, Error>(alloy::primitives::FixedBytes(bytes))
            };
            let header = decode_packet_header(&message.packet_header)?;
            Ok(IEndpointReceiveV1::lzReceiveCall {
                origin: OriginV1 {
                    srcEid: header.source_eid,
                    sender: alloy::primitives::FixedBytes(header.sender),
                    nonce: header.nonce,
                },
                receiver: crate::evm::parse_address(&message.receiver)?,
                guid: bytes32(&message.guid, "guid")?,
                message: hex::decode(message.message.trim_start_matches("0x"))
                    .map_err(|error| Error::InvalidInput(format!("invalid message hex: {error}")))?
                    .into(),
                extraData: Default::default(),
            }
            .abi_encode())
        }
        OperationV1::DeployEvmOft { .. } => Err(Error::InvalidInput(
            "deployment_operation: EVM deployment binds init code, not call calldata".into(),
        )),
        _ => Err(Error::InvalidInput(
            "stellar_only_operation: no EVM calldata exists for this operation".into(),
        )),
    }
}
