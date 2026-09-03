//! Governance boundaries: deterministic testnet proposal and signature
//! paths, and the recovery capability matrix. Mainnet plan, proposal,
//! signature attach, and ingest paths hard-fail with a policy error.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

use crate::domain::{Environment, ExecutablePlanV1, ProposalV1, Vm, SCHEMA_VERSION};
use crate::error::{Error, Result};

/// Canonical policy message for every mainnet mutation path.
pub const PRODUCTION_MUTATION_UNSUPPORTED_V1: &str = "production_mutation_unsupported_v1";

fn require_testnet(environment: Environment) -> Result<()> {
    if environment.is_mainnet() {
        return Err(crate::policy_error(
            PRODUCTION_MUTATION_UNSUPPORTED_V1.to_string(),
        ));
    }
    Ok(())
}

/// Typed verification data returned for a signed proposal.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SignatureVerificationDataV1 {
    pub route_id: String,
    pub sender: String,
    pub nonce_or_sequence: u64,
    pub unsigned_payload: String,
    pub unsigned_payload_sha256: String,
    pub plan_sha256: String,
    pub expires_at_unix: u64,
}

/// Recovery scenario the matrix classifies.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryScenario {
    /// Outbound is contained; restore is required.
    OutboundContained,
    /// Packet never reached the destination.
    PacketUndelivered,
    /// Packet delivered but not acknowledged.
    PacketDeliveredUnacknowledged,
    /// Packet acknowledged but value not settled.
    PacketAcknowledgedUnsettled,
}

/// Mechanism a recovery would use.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryMechanism {
    /// Restore outbound libraries from the containment snapshot.
    OperatorRestore,
    /// Resend the packet through the operator.
    OperatorResend,
    /// Refund the packet value through the operator.
    OperatorRefund,
    /// Withdraw stuck value through the owner after the timelock.
    OwnerTimelockWithdraw,
}

/// One cell of the recovery capability matrix.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryCapabilityV1 {
    /// No v1 recovery path exists for this cell.
    Unavailable { reason: String },
    /// Recovery is authorized through the mechanism.
    Authorized {
        mechanism: RecoveryMechanism,
        requires_source_account: bool,
    },
}

/// Pure recovery capability matrix over the intervening VM and scenario.
pub fn recovery_capability(vm: Vm, scenario: RecoveryScenario) -> RecoveryCapabilityV1 {
    match scenario {
        RecoveryScenario::OutboundContained => RecoveryCapabilityV1::Authorized {
            mechanism: RecoveryMechanism::OperatorRestore,
            requires_source_account: true,
        },
        RecoveryScenario::PacketUndelivered => RecoveryCapabilityV1::Authorized {
            mechanism: RecoveryMechanism::OperatorResend,
            requires_source_account: true,
        },
        RecoveryScenario::PacketDeliveredUnacknowledged => RecoveryCapabilityV1::Unavailable {
            reason: "await_native_acknowledgement_v1".to_string(),
        },
        RecoveryScenario::PacketAcknowledgedUnsettled => match vm {
            Vm::Stellar => RecoveryCapabilityV1::Authorized {
                mechanism: RecoveryMechanism::OperatorRefund,
                requires_source_account: true,
            },
            Vm::Evm => RecoveryCapabilityV1::Authorized {
                mechanism: RecoveryMechanism::OwnerTimelockWithdraw,
                requires_source_account: false,
            },
        },
    }
}

fn validate_plan(plan: &ExecutablePlanV1) -> Result<()> {
    if plan.schema_name != "executable_plan" || plan.schema_version != SCHEMA_VERSION {
        return Err(Error::InvalidInput(
            "unsupported executable plan schema".into(),
        ));
    }
    if plan.route_id.trim().is_empty() {
        return Err(Error::InvalidInput(
            "plan route_id must not be empty".into(),
        ));
    }
    if plan.unsigned_payload.trim().is_empty() {
        return Err(Error::InvalidInput(
            "plan unsigned_payload must not be empty".into(),
        ));
    }
    Ok(())
}

/// Builds a proposal for a plan. Testnet only; mainnet hard-fails.
pub fn build_proposal(environment: Environment, plan: ExecutablePlanV1) -> Result<ProposalV1> {
    require_testnet(environment)?;
    validate_plan(&plan)?;
    Ok(ProposalV1 {
        schema_name: "proposal".to_string(),
        schema_version: SCHEMA_VERSION,
        plan,
        signatures: BTreeMap::new(),
    })
}

/// Attaches (or deterministically replaces) one signer signature. Testnet
/// only; mainnet hard-fails.
pub fn attach_signature(
    environment: Environment,
    proposal: &ProposalV1,
    signer: &str,
    signature: &str,
) -> Result<ProposalV1> {
    require_testnet(environment)?;
    if signer.trim().is_empty() {
        return Err(Error::InvalidInput("signer must not be empty".into()));
    }
    if signature.trim().is_empty() {
        return Err(Error::InvalidInput("signature must not be empty".into()));
    }
    validate_plan(&proposal.plan)?;
    let mut attached = proposal.clone();
    attached
        .signatures
        .insert(signer.to_string(), signature.to_string());
    Ok(attached)
}

/// Ingests a proposal into typed verification data. Testnet only; mainnet
/// hard-fails.
pub fn signature_verification_data(
    environment: Environment,
    proposal: &ProposalV1,
) -> Result<SignatureVerificationDataV1> {
    require_testnet(environment)?;
    validate_plan(&proposal.plan)?;
    let plan_sha256 = crate::canonical_sha256(&proposal.plan)?;
    let unsigned_payload_sha256 =
        hex::encode(Sha256::digest(proposal.plan.unsigned_payload.as_bytes()));
    Ok(SignatureVerificationDataV1 {
        route_id: proposal.plan.route_id.clone(),
        sender: proposal.plan.sender.clone(),
        nonce_or_sequence: proposal.plan.nonce_or_sequence,
        unsigned_payload: proposal.plan.unsigned_payload.clone(),
        unsigned_payload_sha256,
        plan_sha256,
        expires_at_unix: proposal.plan.expires_at_unix,
    })
}

/// Checked native adapter trait for governance paths.
pub trait GovernancePolicyAdapter {
    fn plan_recovery(
        &self,
        environment: Environment,
        vm: Vm,
        scenario: RecoveryScenario,
    ) -> Result<RecoveryCapabilityV1>;
    fn build_proposal(
        &self,
        environment: Environment,
        plan: ExecutablePlanV1,
    ) -> Result<ProposalV1>;
    fn attach_signature(
        &self,
        environment: Environment,
        proposal: &ProposalV1,
        signer: &str,
        signature: &str,
    ) -> Result<ProposalV1>;
    fn signature_verification_data(
        &self,
        environment: Environment,
        proposal: &ProposalV1,
    ) -> Result<SignatureVerificationDataV1>;
}

/// Checked adapter implementation of [GovernancePolicyAdapter].
#[derive(Debug, Default)]
pub struct CheckedGovernancePolicy;

impl GovernancePolicyAdapter for CheckedGovernancePolicy {
    fn plan_recovery(
        &self,
        environment: Environment,
        vm: Vm,
        scenario: RecoveryScenario,
    ) -> Result<RecoveryCapabilityV1> {
        require_testnet(environment)?;
        Ok(recovery_capability(vm, scenario))
    }

    fn build_proposal(
        &self,
        environment: Environment,
        plan: ExecutablePlanV1,
    ) -> Result<ProposalV1> {
        build_proposal(environment, plan)
    }

    fn attach_signature(
        &self,
        environment: Environment,
        proposal: &ProposalV1,
        signer: &str,
        signature: &str,
    ) -> Result<ProposalV1> {
        attach_signature(environment, proposal, signer, signature)
    }

    fn signature_verification_data(
        &self,
        environment: Environment,
        proposal: &ProposalV1,
    ) -> Result<SignatureVerificationDataV1> {
        signature_verification_data(environment, proposal)
    }
}

use crate::domain::{OperationDraftV1, RouteStateV1};
use crate::output::CommandData;
use crate::state::{read_json, RouteStore};
use std::path::Path;

fn route_environment(state_path: &Path) -> Result<(RouteStateV1, RouteStore)> {
    let store = RouteStore::open(state_path)?;
    let state = store.load_state()?;
    Ok((state, store))
}

/// `proposal create`: re-derives a fresh testnet plan from a draft. The
/// draft's operation bytes are never trusted; only its identity.
pub fn create_proposal(state_path: &Path, draft_path: &Path, out: &Path) -> Result<CommandData> {
    let (state, store) = route_environment(state_path)?;
    crate::environment::require_testnet(&state.identity)?;
    let draft: OperationDraftV1 = read_json(draft_path)?;
    if draft.route_id != state.route_id || draft.desired_sha256 != state.desired_sha256 {
        return Err(Error::Conflict(
            "draft does not bind to this route state".into(),
        ));
    }
    let operation_sha256 = crate::canonical_sha256(&draft.operation)?;
    let plan = ExecutablePlanV1 {
        schema_name: "executable_plan".into(),
        schema_version: SCHEMA_VERSION,
        route_id: state.route_id.clone(),
        desired_sha256: state.desired_sha256.clone(),
        operation: draft.operation,
        sender: String::new(),
        nonce_or_sequence: 0,
        unsigned_payload: format!("plan:{operation_sha256}"),
        simulation_sha256: String::new(),
        expires_at_unix: 0,
    };
    let proposal = build_proposal(state.identity.environment, plan)?;
    let relative = Path::new("proposals").join(
        out.file_name()
            .ok_or_else(|| Error::InvalidInput("proposal output must name a file".into()))?,
    );
    store.write_proposal(&relative, &operation_sha256, &proposal)?;
    Ok(CommandData {
        result: serde_json::to_value(&proposal)?,
        artifact: None,
    })
}

/// `--proposal-out`: serialize an operation as a testnet proposal.
pub fn proposal_for_operation(
    state_path: &Path,
    operation: crate::domain::OperationV1,
    _out: &Path,
) -> Result<CommandData> {
    let (state, _store) = route_environment(state_path)?;
    crate::environment::require_testnet(&state.identity)?;
    let operation_sha256 = crate::canonical_sha256(&operation)?;
    let plan = ExecutablePlanV1 {
        schema_name: "executable_plan".into(),
        schema_version: SCHEMA_VERSION,
        route_id: state.route_id.clone(),
        desired_sha256: state.desired_sha256.clone(),
        operation,
        sender: String::new(),
        nonce_or_sequence: 0,
        unsigned_payload: format!("plan:{operation_sha256}"),
        simulation_sha256: String::new(),
        expires_at_unix: 0,
    };
    let proposal = build_proposal(state.identity.environment, plan)?;
    Ok(CommandData {
        result: serde_json::to_value(&proposal)?,
        artifact: None,
    })
}

/// `proposal ingest`: testnet only; execution evidence ingest requires a
/// qualified live adapter and is fail-closed in v1.
pub fn ingest_proposal(
    state_path: &Path,
    _proposal_path: &Path,
    _executed_tx: &str,
    _write: bool,
) -> Result<CommandData> {
    let (state, _store) = route_environment(state_path)?;
    crate::environment::require_testnet(&state.identity)?;
    Err(Error::Chain(
        "proposal ingest requires a qualified live execution-evidence adapter".into(),
    ))
}

/// `proposal stellar-signature attach`: testnet only; writes a new proposal file.
pub fn attach_signature_command(
    state_path: &Path,
    proposal_path: &Path,
    public_key: &str,
    signature: &str,
    out: &Path,
) -> Result<CommandData> {
    let (state, _store) = route_environment(state_path)?;
    crate::environment::require_testnet(&state.identity)?;
    let proposal: ProposalV1 = read_json(proposal_path)?;
    let attached = attach_signature(state.identity.environment, &proposal, public_key, signature)?;
    crate::state::write_create_new_json(out, &attached)?;
    Ok(CommandData {
        result: serde_json::to_value(&attached)?,
        artifact: None,
    })
}

/// `proposal stellar-signature verify`: read-only typed verification data.
pub fn verify_stellar_proposal(state_path: &Path, proposal_path: &Path) -> Result<CommandData> {
    let (state, _store) = route_environment(state_path)?;
    crate::environment::require_testnet(&state.identity)?;
    let proposal: ProposalV1 = read_json(proposal_path)?;
    let data = signature_verification_data(state.identity.environment, &proposal)?;
    Ok(CommandData {
        result: serde_json::to_value(&data)?,
        artifact: None,
    })
}

/// `proposal safe verify`: Safe threshold verification requires a live
/// adapter and is fail-closed in v1.
pub fn verify_safe_proposal(
    state_path: &Path,
    _proposal_path: &Path,
    _safe_tx: &Path,
) -> Result<CommandData> {
    let (state, _store) = route_environment(state_path)?;
    crate::environment::require_testnet(&state.identity)?;
    Err(Error::Chain(
        "Safe proposal verification requires a qualified live Safe adapter".into(),
    ))
}
