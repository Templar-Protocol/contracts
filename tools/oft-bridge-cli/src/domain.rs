use std::{collections::BTreeMap, path::PathBuf, str::FromStr};

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

pub const SCHEMA_VERSION: u32 = 1;
pub const SHARED_DECIMALS: u8 = 6;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Environment {
    StellarTestnetSepolia,
    StellarMainnetEthereum,
}

impl Environment {
    pub const fn is_mainnet(self) -> bool {
        matches!(self, Self::StellarMainnetEthereum)
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Vm {
    Stellar,
    Evm,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    StellarToEvm,
    EvmToStellar,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AssetKind {
    NativeSac,
    IssuedSep41,
    TestOnly,
    Usdc,
}

impl FromStr for AssetKind {
    type Err = Error;

    fn from_str(value: &str) -> Result<Self> {
        match value {
            "native_sac" => Ok(Self::NativeSac),
            "issued_sep41" => Ok(Self::IssuedSep41),
            "test_only" => Ok(Self::TestOnly),
            "usdc" => Ok(Self::Usdc),
            _ => Err(Error::InvalidInput(format!(
                "unsupported asset kind {value}"
            ))),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AssetPolicyV1 {
    pub kind: AssetKind,
    pub asset_id: String,
    pub local_decimals: u8,
    #[serde(default)]
    pub issuer_custodian_evidence_sha256: Option<String>,
    #[serde(default)]
    pub destination_acceptance_evidence_sha256: Option<String>,
    #[serde(default)]
    pub custody_risk_acceptance_sha256: Option<String>,
    #[serde(default)]
    pub forbidden_classic_issuer: Option<String>,
    #[serde(default)]
    pub evidence: BTreeMap<String, String>,
}

impl AssetPolicyV1 {
    pub fn parse(self) -> Result<Self> {
        if self.kind == AssetKind::Usdc || is_known_usdc(&self.asset_id) {
            return Err(Error::Policy("unsupported_use_cctp".into()));
        }
        if self.local_decimals < SHARED_DECIMALS {
            return Err(Error::InvalidInput(format!(
                "local decimals {} are below shared decimals {SHARED_DECIMALS}",
                self.local_decimals
            )));
        }
        if self.asset_id.trim().is_empty() {
            return Err(Error::InvalidInput("asset_id must not be empty".into()));
        }
        Ok(self)
    }
}

pub fn is_known_usdc(asset: &str) -> bool {
    matches!(
        asset.to_ascii_lowercase().as_str(),
        "usdc" | "0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48"
    )
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ChainIdentityV1 {
    pub environment: Environment,
    pub stellar_passphrase: String,
    pub stellar_eid: u32,
    pub stellar_endpoint: String,
    pub stellar_endpoint_code_hash: String,
    pub evm_chain_id: u64,
    pub evm_eid: u32,
    pub evm_endpoint: String,
    pub evm_endpoint_code_hash: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct DesiredRouteV1 {
    pub schema_name: String,
    pub schema_version: u32,
    pub route_id: String,
    pub identity: ChainIdentityV1,
    pub asset: AssetPolicyV1,
    pub stellar_owner: String,
    pub stellar_delegate: String,
    pub evm_owner: String,
    pub evm_delegate: String,
    #[serde(default)]
    pub config: BTreeMap<String, serde_json::Value>,
}

impl DesiredRouteV1 {
    pub fn parse(mut self) -> Result<Self> {
        if self.schema_name != "desired_route" || self.schema_version != SCHEMA_VERSION {
            return Err(Error::InvalidInput(
                "unsupported desired route schema".into(),
            ));
        }
        if self.route_id.trim().is_empty() {
            return Err(Error::InvalidInput("route_id must not be empty".into()));
        }
        self.asset = self.asset.parse()?;
        Ok(self)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ArtifactRefV1 {
    pub kind: String,
    pub path: PathBuf,
    pub sha256: String,
    pub schema_version: u32,
    pub authoritative: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct OpeningCustodyV1 {
    pub stellar_ledger: u64,
    pub stellar_ledger_hash: String,
    pub lockbox_raw: u128,
    pub evm_block: u64,
    pub evm_block_hash: String,
    pub evm_supply_raw: u128,
    pub history_evidence_sha256: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RouteStateV1 {
    pub schema_name: String,
    pub schema_version: u32,
    pub route_id: String,
    pub desired_sha256: String,
    pub identity: ChainIdentityV1,
    pub asset: AssetPolicyV1,
    pub opening_custody: Option<OpeningCustodyV1>,
    pub operations_log: PathBuf,
    pub messages_log: PathBuf,
    pub lock_file: PathBuf,
    #[serde(default)]
    pub contracts: BTreeMap<String, String>,
    #[serde(default)]
    pub effective_config: BTreeMap<String, serde_json::Value>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum OperationV1 {
    InstallStellarWasm {
        wasm_sha256: String,
    },
    DeployStellarOft {
        salt: String,
    },
    DeployEvmOft {
        deployer: String,
        nonce: u64,
    },
    BeginStellarOwnershipTransfer {
        new_owner: String,
        ttl: u32,
    },
    AcceptStellarOwnership,
    CancelStellarOwnershipTransfer,
    TransferEvmOwnership {
        new_owner: String,
    },
    SetStellarDelegate {
        delegate: String,
    },
    SetEvmDelegate {
        delegate: String,
    },
    SetStellarPeer {
        remote_eid: u32,
        peer: String,
    },
    SetEvmPeer {
        remote_eid: u32,
        peer: String,
    },
    SetStellarSendLibrary {
        remote_eid: u32,
        library: String,
    },
    SetStellarReceiveLibrary {
        remote_eid: u32,
        library: String,
        grace_period_seconds: u64,
    },
    RemoveStellarReceiveLibraryTimeout {
        remote_eid: u32,
    },
    SetEvmSendLibrary {
        remote_eid: u32,
        library: String,
    },
    SetEvmReceiveLibrary {
        remote_eid: u32,
        library: String,
        grace_period_seconds: u64,
    },
    RemoveEvmReceiveLibraryTimeout {
        remote_eid: u32,
    },
    SetStellarUlnConfig {
        remote_eid: u32,
        config_sha256: String,
    },
    SetEvmUlnConfig {
        remote_eid: u32,
        config_sha256: String,
    },
    SetStellarExecutorConfig {
        remote_eid: u32,
        config_sha256: String,
    },
    SetEvmExecutorConfig {
        remote_eid: u32,
        config_sha256: String,
    },
    SetStellarReceiveOptions {
        remote_eid: u32,
        message_type: u16,
        options: String,
    },
    SetEvmReceiveOptions {
        remote_eid: u32,
        message_type: u16,
        options: String,
    },
    SetDefaultFee {
        bps: u32,
    },
    SetDestinationFee {
        remote_eid: u32,
        bps: u32,
    },
    SetFeeRecipient {
        recipient: String,
    },
    SetMessageInspector {
        inspector: Option<String>,
    },
    SetInboundRateLimit {
        remote_eid: u32,
        limit_raw: u128,
        window_seconds: u64,
        mode: String,
    },
    SetOutboundRateLimit {
        remote_eid: u32,
        limit_raw: u128,
        window_seconds: u64,
        mode: String,
    },
    PauseEmergency,
    UnpauseEmergency,
    SetTtlConfig {
        instance_threshold: u32,
        instance_extend_to: u32,
        persistent_threshold: u32,
        persistent_extend_to: u32,
    },
    FreezeTtlConfig {
        acknowledgement: String,
    },
    ExtendInstanceTtl {
        ledgers: u32,
    },
    GrantRole {
        role: String,
        address: String,
    },
    RevokeRole {
        role: String,
        address: String,
    },
    SetRoleAdmin {
        role: String,
        admin_role: String,
    },
    RemoveRoleAdmin {
        role: String,
        admin_role: String,
    },
    SendLeg {
        intent_sha256: String,
    },
    CommitVerification {
        vm: Vm,
        guid: String,
        packet_sha256: String,
    },
    ExecuteReceive {
        vm: Vm,
        guid: String,
        packet_sha256: String,
    },
    ContainOutbound {
        direction: Direction,
    },
    RestoreOutbound {
        snapshot_sha256: String,
    },
    RestoreFootprint {
        original_operation_sha256: String,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum LocalPreparationV1 {
    BindRoute { desired_sha256: String },
    BuildArtifact { artifact_lock_sha256: String },
    AdoptRoute { opening_custody_sha256: String },
    ImportEvidence { evidence_sha256: String },
}

/// Typed, route-bound leg intent produced by `leg quote` and consumed by
/// `leg send`. Amounts are decimal strings per the schema rule.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct LegIntentV1 {
    pub schema_name: String,
    pub schema_version: u32,
    pub route_id: String,
    pub desired_sha256: String,
    pub direction: Direction,
    pub amount_raw: String,
    pub to: String,
}

impl LegIntentV1 {
    pub fn parse(self) -> Result<Self> {
        if self.schema_name != "leg_intent" || self.schema_version != SCHEMA_VERSION {
            return Err(Error::InvalidInput("unsupported leg intent schema".into()));
        }
        if self.route_id.trim().is_empty() || self.desired_sha256.trim().is_empty() {
            return Err(Error::InvalidInput(
                "leg intent must bind a route and desired digest".into(),
            ));
        }
        let amount: u128 = self
            .amount_raw
            .parse()
            .map_err(|_| Error::InvalidInput("amount_raw must be a decimal string".into()))?;
        if amount == 0 {
            return Err(Error::InvalidInput(
                "amount_raw must be greater than zero".into(),
            ));
        }
        if self.to.trim().is_empty() {
            return Err(Error::InvalidInput("destination must not be empty".into()));
        }
        Ok(self)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct OperationDraftV1 {
    pub schema_name: String,
    pub schema_version: u32,
    pub route_id: String,
    pub desired_sha256: String,
    pub operation: OperationV1,
    pub observed_sha256: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct StellarPlanBindingV1 {
    pub network_passphrase: String,
    pub source_account: String,
    /// Decimal string per schema rule for sequences.
    pub sequence: String,
    pub min_ledger: u32,
    pub max_ledger: u32,
    /// Soroban authorization entries; v1 only allows source-account entries.
    pub auth_entries: Vec<String>,
    /// Base64 transaction envelope XDR; empty until a live adapter constructs it.
    pub envelope_xdr: String,
    pub envelope_sha256: String,
    pub simulation_ledger: u32,
    /// Live signer weights keyed by public key.
    pub signer_weights: BTreeMap<String, u32>,
    pub required_threshold_weight: u32,
    pub threshold_level: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SafeTransactionV1 {
    pub to: String,
    /// Decimal string wei value.
    pub value: String,
    pub data: String,
    pub operation: u8,
    pub safe_tx_gas: String,
    pub base_gas: String,
    pub gas_price: String,
    pub gas_token: String,
    pub refund_receiver: String,
    /// Decimal string Safe nonce.
    pub nonce: String,
    pub threshold: u32,
    pub safe_tx_hash: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct EvmPlanBindingV1 {
    /// Decimal string chain ID.
    pub chain_id: String,
    pub target: String,
    /// Decimal string wei value.
    pub value: String,
    /// Decimal string account nonce.
    pub nonce: String,
    pub calldata: String,
    pub gas_limit: String,
    pub max_fee_per_gas_wei: String,
    pub max_priority_fee_per_gas_wei: String,
    pub transaction_digest: String,
    #[serde(default)]
    pub safe: Option<SafeTransactionV1>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ExecutablePlanV1 {
    pub schema_name: String,
    pub schema_version: u32,
    pub route_id: String,
    pub desired_sha256: String,
    pub operation: OperationV1,
    pub artifact_lock_sha256: String,
    pub simulation_sha256: String,
    pub expires_at_unix: u64,
    #[serde(default)]
    pub stellar: Option<StellarPlanBindingV1>,
    #[serde(default)]
    pub evm: Option<EvmPlanBindingV1>,
    /// Digest of the journal checkpoint naming the next safe continuation.
    pub continuation_sha256: String,
}

/// Monotonic packet lifecycle stage. Only `Reobserved` may revisit evidence.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageStageV1 {
    ForwardSourceAccepted,
    ForwardLocked,
    ForwardMinted,
    ReverseSourceAccepted,
    ReverseBurned,
    ReverseUnlocked,
    Reobserved,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct MessageStatusEventV1 {
    pub stage: MessageStageV1,
    pub observed_at_unix: u64,
    pub evidence_sha256: String,
}

/// Append-only packet record keyed by `(source_eid, sender, nonce, guid)`.
/// Packet header/message bytes and the effective security-stack snapshot at
/// send time are immutable; only status events append.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct MessageRecordV1 {
    pub schema_name: String,
    pub schema_version: u32,
    pub source_eid: u32,
    pub sender: String,
    /// Decimal string packet nonce.
    pub nonce: String,
    pub guid: String,
    pub direction: Direction,
    /// Decimal string raw amount in shared units.
    pub amount_raw: String,
    pub packet_sha256: String,
    pub payload_sha256: String,
    pub origin: String,
    pub receiver: String,
    /// Effective config snapshot at send time; immutable after append.
    pub config_snapshot_sha256: String,
    pub source_transaction: String,
    pub destination_transaction: Option<String>,
    pub status_events: Vec<MessageStatusEventV1>,
}

impl MessageRecordV1 {
    pub fn identity(&self) -> (u32, String, String, String) {
        (
            self.source_eid,
            self.sender.clone(),
            self.nonce.clone(),
            self.guid.clone(),
        )
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ProposalV1 {
    pub schema_name: String,
    pub schema_version: u32,
    pub plan: ExecutablePlanV1,
    #[serde(default)]
    pub signatures: BTreeMap<String, String>,
}
