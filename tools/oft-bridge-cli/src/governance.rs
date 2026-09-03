//! Governance boundaries: deterministic testnet proposal and signature
//! paths, and the recovery capability matrix. Mainnet plan, proposal,
//! signature attach, and ingest paths hard-fail with a policy error.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

use crate::domain::{Environment, ExecutablePlanV1, OperationV1, ProposalV1, Vm, SCHEMA_VERSION};
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
    pub vm: Vm,
    pub sender: String,
    /// Decimal string sequence (Stellar) or nonce (EVM).
    pub sequence_or_nonce: String,
    pub unsigned_payload: String,
    pub unsigned_payload_sha256: String,
    pub plan_sha256: String,
    pub expires_at_unix: u64,
}

/// Outcome of transaction preparation when a qualified simulation demands
/// footprint restoration before the original operation can proceed.
#[derive(Debug)]
pub enum PrepareOutcome {
    Ready(ExecutablePlanV1),
    RestorationRequired(OperationV1),
}

/// Derives the journaled footprint-restore sub-operation for an original
/// operation. The original plan must be rebuilt afterwards because
/// restoration consumes a sequence.
pub fn restoration_required(original: &OperationV1) -> Result<OperationV1> {
    Ok(OperationV1::RestoreFootprint {
        original_operation_sha256: crate::canonical_sha256(original)?,
    })
}

/// Maps a typed adapter refusal carrying [`crate::stellar::RESTORATION_REQUIRED`]
pub fn prepare_from_simulation(
    original: &OperationV1,
    simulation_error: Error,
) -> Result<PrepareOutcome> {
    let marker = format!(": {}", crate::stellar::RESTORATION_REQUIRED);
    match &simulation_error {
        Error::Chain(message) if message.ends_with(&marker) => Ok(
            PrepareOutcome::RestorationRequired(restoration_required(original)?),
        ),
        _ => Err(simulation_error),
    }
}

/// Live HTTP adapters for proposal construction. `None` on a mutation path
/// means the command cannot bind live state and fails closed.
pub struct LiveAdapters<'a> {
    pub stellar_url: Option<&'a str>,
    pub evm_url: Option<&'a str>,
}

fn require_url<'a>(url: Option<&'a str>, chain: &str) -> Result<&'a str> {
    url.ok_or_else(|| {
        Error::InvalidInput(format!(
            "live_environment_required: {chain} RPC URL is required to bind a proposal"
        ))
    })
}

/// Builds an executable plan bound to live adapter state: Stellar proposals
/// bind passphrase, source account, sequence, time bounds and the authority
/// topology (signer weights and required threshold); EVM proposals bind
/// chain ID, nonce, typed calldata and a conservative gas policy, plus Safe
/// metadata when the owner is a Safe. Offline tests inject fakes.
pub fn build_executable_plan(
    state: &RouteStateV1,
    operation: &OperationV1,
    stellar: &dyn crate::stellar::StellarChain,
    evm: &dyn crate::evm::EvmChain,
) -> Result<ExecutablePlanV1> {
    let operation_sha256 = crate::canonical_sha256(operation)?;
    let desired_sha256 = state.desired_sha256.clone();
    match operation_vm(operation) {
        Vm::Stellar => {
            let binding = stellar_plan_binding(state, operation, &operation_sha256, stellar)?;
            executable_plan(
                state,
                operation,
                &operation_sha256,
                desired_sha256,
                Some(binding),
                None,
            )
        }
        Vm::Evm => {
            let binding = evm_plan_binding(state, operation, evm)?;
            executable_plan(
                state,
                operation,
                &operation_sha256,
                desired_sha256,
                None,
                Some(binding),
            )
        }
    }
}

/// Shared plan envelope; the simulation digest binds the offline-construction
/// marker plus the operation digest.
fn executable_plan(
    state: &RouteStateV1,
    operation: &OperationV1,
    operation_sha256: &str,
    desired_sha256: String,
    stellar_binding: Option<crate::domain::StellarPlanBindingV1>,
    evm_binding: Option<crate::domain::EvmPlanBindingV1>,
) -> Result<ExecutablePlanV1> {
    Ok(ExecutablePlanV1 {
        schema_name: "executable_plan".into(),
        schema_version: SCHEMA_VERSION,
        route_id: state.route_id.clone(),
        desired_sha256,
        operation: operation.clone(),
        artifact_lock_sha256: crate::artifacts::lock_sha256()?,
        simulation_sha256: crate::canonical_sha256(&serde_json::json!({
            "simulated": false,
            "reason": "offline_testnet_proposal_construction",
            "operation_sha256": operation_sha256,
        }))?,
        expires_at_unix: 0,
        stellar: stellar_binding,
        evm: evm_binding,
        continuation_sha256: String::new(),
    })
}

fn stellar_plan_binding(
    state: &RouteStateV1,
    operation: &OperationV1,
    operation_sha256: &str,
    stellar: &dyn crate::stellar::StellarChain,
) -> Result<crate::domain::StellarPlanBindingV1> {
    let passphrase = stellar.network_passphrase()?;
    if passphrase != state.identity.stellar_passphrase {
        return Err(Error::Policy(
            "derived environment mismatch: RPC passphrase differs from route identity".into(),
        ));
    }
    let sender = route_owner(state, Vm::Stellar)?;
    let sequence = stellar.account_sequence(&sender)?;
    let ledger = stellar.latest_ledger()?;
    let weights = stellar.account_signers(&sender)?;
    let level = threshold_level(operation);
    let threshold = stellar.account_threshold(&sender, level)?;
    let available: u64 = weights.values().map(|weight| u64::from(*weight)).sum();
    if available < u64::from(threshold) {
        return Err(Error::Policy(format!(
            "insufficient signer weight {available} for threshold {threshold} ({level})"
        )));
    }
    let marker = serde_json::json!({
        "envelope": "unconstructed",
        "operation_sha256": operation_sha256,
        "sequence": sequence,
    });
    Ok(crate::domain::StellarPlanBindingV1 {
        network_passphrase: passphrase,
        source_account: sender,
        sequence,
        min_ledger: ledger,
        max_ledger: ledger.saturating_add(1_000),
        auth_entries: Vec::new(),
        envelope_xdr: String::new(),
        envelope_sha256: crate::canonical_sha256(&marker)?,
        simulation_ledger: ledger,
        signer_weights: weights,
        required_threshold_weight: threshold,
        threshold_level: level.to_string(),
    })
}

fn evm_plan_binding(
    state: &RouteStateV1,
    operation: &OperationV1,
    evm: &dyn crate::evm::EvmChain,
) -> Result<crate::domain::EvmPlanBindingV1> {
    let chain_id = crate::block_on_result(evm.chain_id())?;
    if chain_id != state.identity.evm_chain_id {
        return Err(Error::Policy(
            "derived environment mismatch: RPC chain id differs from route identity".into(),
        ));
    }
    let owner = route_owner(state, Vm::Evm)?;
    let address = crate::evm::parse_address(&owner)?;
    let nonce = crate::block_on_result(evm.account_nonce(address))?;
    let calldata = crate::layerzero::encode_calldata(operation)?;
    let safe_binding =
        crate::block_on_result(evm.safe_state(address))?.map(|(threshold, safe_nonce)| {
            crate::domain::SafeTransactionV1 {
                to: state.contracts.get("evm_oft").cloned().unwrap_or_default(),
                value: "0".into(),
                data: hex::encode(&calldata),
                operation: 0,
                safe_tx_gas: "0".into(),
                base_gas: "0".into(),
                gas_price: "0".into(),
                gas_token: "0x0000000000000000000000000000000000000000".into(),
                refund_receiver: "0x0000000000000000000000000000000000000000".into(),
                nonce: safe_nonce,
                threshold,
                // Computing SafeTxHash requires the qualified live Safe
                // adapter; a locally guessed EIP-712 encoding is worse
                // than an explicit pending marker.
                safe_tx_hash: "pending_qualified_adapter".into(),
            }
        });
    let binding = crate::domain::EvmPlanBindingV1 {
        chain_id: chain_id.to_string(),
        target: state.contracts.get("evm_oft").cloned().unwrap_or_default(),
        value: "0".into(),
        nonce: nonce.to_string(),
        calldata: format!("0x{}", hex::encode(&calldata)),
        // Explicit constant gas policy marker until live estimate
        // plumbing lands; the digest below binds these exact values.
        gas_limit: "constant:200000".into(),
        max_fee_per_gas_wei: "constant:20000000000".into(),
        max_priority_fee_per_gas_wei: "constant:1000000000".into(),
        transaction_digest: String::new(),
        safe: safe_binding,
    };
    let transaction_digest =
        hex::encode(crate::evm::keccak256_of(&crate::canonical_bytes(&binding)?));
    Ok(crate::domain::EvmPlanBindingV1 {
        transaction_digest,
        ..binding
    })
}

fn route_owner(state: &RouteStateV1, vm: Vm) -> Result<String> {
    let key = match vm {
        Vm::Stellar => "stellar_owner",
        Vm::Evm => "evm_owner",
    };
    state.contracts.get(key).cloned().ok_or_else(|| {
        Error::Custody(format!(
            "route owner '{key}' not recorded; re-adopt the route with authority records"
        ))
    })
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
    if plan.stellar.is_some() == plan.evm.is_some() {
        return Err(Error::InvalidInput(
            "plan must bind exactly one chain".into(),
        ));
    }
    Ok(())
}

/// VM an operation mutates. Dual-sided containment follows the outbound
/// direction; cross-VM recovery operations carry their explicit `vm`.
pub fn operation_vm(operation: &crate::domain::OperationV1) -> Vm {
    use crate::domain::OperationV1;
    match operation {
        OperationV1::DeployEvmOft { .. }
        | OperationV1::TransferEvmOwnership { .. }
        | OperationV1::SetEvmDelegate { .. }
        | OperationV1::SetEvmPeer { .. }
        | OperationV1::SetEvmSendLibrary { .. }
        | OperationV1::SetEvmReceiveLibrary { .. }
        | OperationV1::RemoveEvmReceiveLibraryTimeout { .. }
        | OperationV1::SetEvmUlnConfig { .. }
        | OperationV1::SetEvmExecutorConfig { .. }
        | OperationV1::SetEvmReceiveOptions { .. } => Vm::Evm,
        OperationV1::CommitVerification { vm, .. } | OperationV1::ExecuteReceive { vm, .. } => *vm,
        // Dual-sided containment is driven from the outbound VM first.
        OperationV1::ContainOutbound { .. }
        | OperationV1::RestoreOutbound { .. }
        | OperationV1::InstallStellarWasm { .. }
        | OperationV1::DeployStellarOft { .. }
        | OperationV1::BeginStellarOwnershipTransfer { .. }
        | OperationV1::AcceptStellarOwnership
        | OperationV1::CancelStellarOwnershipTransfer
        | OperationV1::SetStellarDelegate { .. }
        | OperationV1::SetStellarPeer { .. }
        | OperationV1::SetStellarSendLibrary { .. }
        | OperationV1::SetStellarReceiveLibrary { .. }
        | OperationV1::RemoveStellarReceiveLibraryTimeout { .. }
        | OperationV1::SetStellarUlnConfig { .. }
        | OperationV1::SetStellarExecutorConfig { .. }
        | OperationV1::SetStellarReceiveOptions { .. }
        | OperationV1::SetDefaultFee { .. }
        | OperationV1::SetDestinationFee { .. }
        | OperationV1::SetFeeRecipient { .. }
        | OperationV1::SetMessageInspector { .. }
        | OperationV1::SetInboundRateLimit { .. }
        | OperationV1::SetOutboundRateLimit { .. }
        | OperationV1::PauseEmergency
        | OperationV1::UnpauseEmergency
        | OperationV1::SetTtlConfig { .. }
        | OperationV1::FreezeTtlConfig { .. }
        | OperationV1::ExtendInstanceTtl { .. }
        | OperationV1::GrantRole { .. }
        | OperationV1::RevokeRole { .. }
        | OperationV1::SetRoleAdmin { .. }
        | OperationV1::RemoveRoleAdmin { .. }
        | OperationV1::SendLeg { .. }
        | OperationV1::RestoreFootprint { .. } => Vm::Stellar,
    }
}

/// Required Stellar threshold level for an operation.
pub fn threshold_level(operation: &crate::domain::OperationV1) -> &'static str {
    use crate::domain::OperationV1;
    match operation {
        OperationV1::BeginStellarOwnershipTransfer { .. }
        | OperationV1::AcceptStellarOwnership
        | OperationV1::CancelStellarOwnershipTransfer => "high",
        _ => "medium",
    }
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

/// Derives typed out-of-band signing material from a proposal's chain
/// binding. The unsigned payload is the Stellar envelope XDR when present,
/// otherwise the EVM transaction digest.
pub fn signature_verification_data(
    environment: Environment,
    proposal: &ProposalV1,
) -> Result<SignatureVerificationDataV1> {
    require_testnet(environment)?;
    validate_plan(&proposal.plan)?;
    let (vm, sender, sequence_or_nonce, unsigned_payload) =
        match (proposal.plan.stellar.as_ref(), proposal.plan.evm.as_ref()) {
            (Some(binding), None) => (
                Vm::Stellar,
                binding.source_account.clone(),
                binding.sequence.clone(),
                binding.envelope_xdr.clone(),
            ),
            (None, Some(binding)) => (
                Vm::Evm,
                String::new(),
                binding.nonce.clone(),
                binding.transaction_digest.clone(),
            ),
            _ => {
                return Err(Error::InvalidInput(
                    "plan must bind exactly one chain".into(),
                ))
            }
        };
    if unsigned_payload.trim().is_empty() {
        return Err(Error::Chain(
            "proposal has no unsigned payload yet; construct the envelope via the qualified adapter".into(),
        ));
    }
    let unsigned_payload_sha256 = hex::encode(Sha256::digest(unsigned_payload.as_bytes()));
    let plan_sha256 = crate::canonical_sha256(&proposal.plan)?;
    Ok(SignatureVerificationDataV1 {
        route_id: proposal.plan.route_id.clone(),
        vm,
        sender,
        sequence_or_nonce,
        unsigned_payload,
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
pub fn create_proposal(
    state_path: &Path,
    draft_path: &Path,
    out: &Path,
    stellar_rpc: Option<&str>,
    evm_rpc: Option<&str>,
) -> Result<CommandData> {
    let (state, store) = route_environment(state_path)?;
    crate::environment::require_testnet(&state.identity)?;
    let draft: OperationDraftV1 = read_json(draft_path)?;
    if draft.route_id != state.route_id || draft.desired_sha256 != state.desired_sha256 {
        return Err(Error::Conflict(
            "draft does not bind to this route state".into(),
        ));
    }
    let plan = with_live_adapters(stellar_rpc, evm_rpc, |stellar, evm| {
        build_executable_plan(&state, &draft.operation, stellar, evm)
    })?;
    let operation_sha256 = crate::canonical_sha256(&draft.operation)?;
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
    operation: &crate::domain::OperationV1,
    _out: &Path,
    stellar_rpc: Option<&str>,
    evm_rpc: Option<&str>,
) -> Result<CommandData> {
    let (state, _store) = route_environment(state_path)?;
    crate::environment::require_testnet(&state.identity)?;
    let plan = with_live_adapters(stellar_rpc, evm_rpc, |stellar, evm| {
        build_executable_plan(&state, operation, stellar, evm)
    })?;
    let proposal = build_proposal(state.identity.environment, plan)?;
    Ok(CommandData {
        result: serde_json::to_value(&proposal)?,
        artifact: None,
    })
}

/// Builds the concrete HTTP adapter pair used to bind a proposal. Both RPC
/// URLs are mandatory: chain-identity checks read both sides.
fn with_live_adapters<T>(
    stellar_rpc: Option<&str>,
    evm_rpc: Option<&str>,
    build: impl FnOnce(&dyn crate::stellar::StellarChain, &dyn crate::evm::EvmChain) -> Result<T>,
) -> Result<T> {
    let stellar = crate::stellar::HttpStellarChain::new(require_url(stellar_rpc, "Stellar")?)?;
    let evm = crate::evm::HttpEvmChain::new(require_url(evm_rpc, "EVM")?)?;
    build(&stellar, &evm)
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
