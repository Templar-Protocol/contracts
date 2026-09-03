//! Recovery capability matrix and governance mainnet hard-disable tests.

use std::collections::BTreeMap;

use sha2::Digest;
use templar_oft_bridge_cli::domain::Vm;
use templar_oft_bridge_cli::domain::{Environment, ExecutablePlanV1, ProposalV1, SCHEMA_VERSION};
use templar_oft_bridge_cli::error::Error;
use templar_oft_bridge_cli::governance::{
    attach_signature, build_proposal, recovery_capability, signature_verification_data,
    CheckedGovernancePolicy, GovernancePolicyAdapter, RecoveryCapabilityV1, RecoveryMechanism,
    RecoveryScenario, PRODUCTION_MUTATION_UNSUPPORTED_V1,
};

fn plan() -> ExecutablePlanV1 {
    ExecutablePlanV1 {
        schema_name: "executable_plan".to_string(),
        schema_version: SCHEMA_VERSION,
        route_id: "route-recovery".to_string(),
        desired_sha256: "desired".to_string(),
        operation: templar_oft_bridge_cli::domain::OperationV1::SendLeg {
            intent_sha256: "intent".to_string(),
        },
        artifact_lock_sha256: "artifact-lock".to_string(),
        simulation_sha256: "sim".to_string(),
        expires_at_unix: 900,
        stellar: Some(templar_oft_bridge_cli::domain::StellarPlanBindingV1 {
            network_passphrase: "Test SDF Network ; September 2015".to_string(),
            source_account: "GSENDER".to_string(),
            sequence: "7".to_string(),
            min_ledger: 1,
            max_ledger: 100,
            auth_entries: Vec::new(),
            envelope_xdr: "payload-bytes".to_string(),
            envelope_sha256: hex::encode(sha2::Sha256::digest(b"payload-bytes")),
            simulation_ledger: 50,
            signer_weights: BTreeMap::new(),
            required_threshold_weight: 1,
            threshold_level: "medium".to_string(),
        }),
        evm: None,
        continuation_sha256: String::new(),
    }
}

fn proposal() -> ProposalV1 {
    ProposalV1 {
        schema_name: "proposal".to_string(),
        schema_version: SCHEMA_VERSION,
        plan: plan(),
        signatures: BTreeMap::new(),
    }
}

#[test]
fn contained_outbound_is_restore_authorized() {
    let capability = recovery_capability(Vm::Stellar, RecoveryScenario::OutboundContained);
    assert_eq!(
        capability,
        RecoveryCapabilityV1::Authorized {
            mechanism: RecoveryMechanism::OperatorRestore,
            requires_source_account: true,
        }
    );
}

#[test]
fn undelivered_packets_are_resend_authorized() {
    for vm in [Vm::Stellar, Vm::Evm] {
        let capability = recovery_capability(vm, RecoveryScenario::PacketUndelivered);
        assert_eq!(
            capability,
            RecoveryCapabilityV1::Authorized {
                mechanism: RecoveryMechanism::OperatorResend,
                requires_source_account: true,
            }
        );
    }
}

#[test]
fn delivered_unacknowledged_is_unavailable() {
    for vm in [Vm::Stellar, Vm::Evm] {
        let capability = recovery_capability(vm, RecoveryScenario::PacketDeliveredUnacknowledged);
        match capability {
            RecoveryCapabilityV1::Unavailable { reason } => {
                assert_eq!(reason, "await_native_acknowledgement_v1");
            }
            other @ RecoveryCapabilityV1::Authorized { .. } => {
                panic!("expected unavailable, got {other:?}")
            }
        }
    }
}

#[test]
fn acknowledged_unsettled_splits_by_vm() {
    let stellar = recovery_capability(Vm::Stellar, RecoveryScenario::PacketAcknowledgedUnsettled);
    assert_eq!(
        stellar,
        RecoveryCapabilityV1::Authorized {
            mechanism: RecoveryMechanism::OperatorRefund,
            requires_source_account: true,
        }
    );
    let evm = recovery_capability(Vm::Evm, RecoveryScenario::PacketAcknowledgedUnsettled);
    assert_eq!(
        evm,
        RecoveryCapabilityV1::Authorized {
            mechanism: RecoveryMechanism::OwnerTimelockWithdraw,
            requires_source_account: false,
        }
    );
}

#[test]
fn mainnet_recovery_path_hard_fails_with_policy_error() {
    let error = CheckedGovernancePolicy
        .plan_recovery(
            Environment::StellarMainnetEthereum,
            Vm::Stellar,
            RecoveryScenario::PacketUndelivered,
        )
        .unwrap_err();
    assert_eq!(error.code(), PRODUCTION_MUTATION_UNSUPPORTED_V1);
}

#[test]
fn mainnet_proposal_and_signature_paths_hard_fail() {
    let environment = Environment::StellarMainnetEthereum;
    let proposal = proposal();
    let built = build_proposal(environment, plan()).unwrap_err();
    assert_eq!(built.code(), PRODUCTION_MUTATION_UNSUPPORTED_V1);
    let attached = attach_signature(environment, &proposal, "GSIGNER", "sig").unwrap_err();
    assert_eq!(attached.code(), PRODUCTION_MUTATION_UNSUPPORTED_V1);
    let verified = signature_verification_data(environment, &proposal).unwrap_err();
    assert_eq!(verified.code(), PRODUCTION_MUTATION_UNSUPPORTED_V1);
}

#[test]
fn testnet_proposal_attach_and_verification_are_deterministic() {
    let environment = Environment::StellarTestnetSepolia;
    let first = build_proposal(environment, plan()).unwrap();
    let second = build_proposal(environment, plan()).unwrap();
    assert_eq!(first, second);
    assert!(first.signatures.is_empty());

    let attached = attach_signature(environment, &first, "GSIGNER", "sig-1").unwrap();
    assert_eq!(
        attached.signatures.get("GSIGNER"),
        Some(&"sig-1".to_string())
    );
    let again = attach_signature(environment, &attached, "GSIGNER", "sig-1").unwrap();
    assert_eq!(attached, again);

    let data = signature_verification_data(environment, &again).unwrap();
    let repeated = signature_verification_data(environment, &again).unwrap();
    assert_eq!(data, repeated);
    assert_eq!(data.route_id, "route-recovery");
    assert_eq!(data.sender, "GSENDER");
    assert_eq!(data.sequence_or_nonce, "7");
    assert_eq!(data.unsigned_payload, "payload-bytes");
    assert_eq!(data.unsigned_payload_sha256.len(), 64);
    assert_eq!(data.plan_sha256.len(), 64);
    assert_eq!(data.expires_at_unix, 900);
}

#[test]
fn signature_attachment_refuses_empty_inputs() {
    let environment = Environment::StellarTestnetSepolia;
    let proposal = proposal();
    let empty_signer = attach_signature(environment, &proposal, " ", "sig").unwrap_err();
    assert!(matches!(empty_signer, Error::InvalidInput(_)));
    let empty_signature = attach_signature(environment, &proposal, "GSIGNER", "").unwrap_err();
    assert!(matches!(empty_signature, Error::InvalidInput(_)));
}

#[test]
fn checked_governance_trait_matches_free_functions() {
    let adapter = CheckedGovernancePolicy;
    let environment = Environment::StellarTestnetSepolia;
    let built = adapter.build_proposal(environment, plan()).unwrap();
    let attached = adapter
        .attach_signature(environment, &built, "GSIGNER", "sig-1")
        .unwrap();
    assert_eq!(
        adapter
            .signature_verification_data(environment, &attached)
            .unwrap(),
        signature_verification_data(environment, &attached).unwrap()
    );
    assert_eq!(
        adapter
            .plan_recovery(
                environment,
                Vm::Stellar,
                RecoveryScenario::OutboundContained
            )
            .unwrap(),
        recovery_capability(Vm::Stellar, RecoveryScenario::OutboundContained)
    );
}
