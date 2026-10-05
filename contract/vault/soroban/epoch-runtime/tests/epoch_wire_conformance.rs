//! Host-free exact-conformance checks for the dedicated epoch deploy target.
//!
//! These tests need no contract runtime host. They pin the epoch wire-decoder
//! rejection law and the exact stable command-surface tags directly against
//! the epoch command profile used by `templar-soroban-epoch-runtime`.

use rstest::rstest;
use templar_soroban_shared_types::{
    CodecError, VaultCommand, EPOCH_PHASE_CUTOFF, EPOCH_PHASE_OPEN, EPOCH_PHASE_SETTLED,
    RUNTIME_EPOCH_FEATURE_FLAGS,
};

/// The dedicated epoch deployment profile advertises exactly the pause and
/// epoch-settlement capabilities: mask `0xA0`. No immediate action bit and no
/// companion-upgrade capability may ever appear on the epoch target.
#[test]
fn epoch_profile_mask_is_exactly_pause_and_epoch_settlement() {
    assert_eq!(RUNTIME_EPOCH_FEATURE_FLAGS, 0xA0);
    assert_eq!(RUNTIME_EPOCH_FEATURE_FLAGS & 0x1F, 0);
    assert_eq!(RUNTIME_EPOCH_FEATURE_FLAGS & 0x40, 0);
    assert_eq!(EPOCH_PHASE_OPEN, 0);
    assert_eq!(EPOCH_PHASE_CUTOFF, 1);
    assert_eq!(EPOCH_PHASE_SETTLED, 2);
}

/// Every immediate-only command tag fails the epoch wire decoder with
/// `InvalidTag`, both as a bare tag byte and with realistic trailing payload
/// bytes. The epoch target's `execute` surfaces decoder rejection as
/// `InvalidInput` through the runtime law.
#[rstest]
#[case(0u8)] // immediate deposit
#[case(3)] // allocate
#[case(4)] // refresh markets
#[case(5)] // fee crystallization
#[case(6)] // atomic exit
#[case(7)] // atomic redeem exit
#[case(8)] // idle resync
#[case(9)] // cancel migration
#[case(10)] // ttl extension
fn immediate_command_tag_fails_decode_with_invalid_tag(#[case] tag: u8) {
    assert_eq!(
        VaultCommand::decode(&[tag]),
        Err(CodecError::InvalidTag),
        "immediate tag {tag} must not decode in the epoch wire format"
    );
    // Same tag with realistic trailing payload fields still fails closed.
    let mut payload = vec![tag; 33];
    payload.extend_from_slice(&[0x01; 32]);
    assert_eq!(VaultCommand::decode(&payload), Err(CodecError::InvalidTag));
}

/// Unknown high tags also fail closed with `InvalidTag`. Tag 22 is the
/// lawful one-time backed seed command, so the first unallocated tag (23)
/// and reserved high tags are the unknown-tag surface.
#[rstest]
#[case(23u8)]
#[case(64)]
#[case(255)]
fn unknown_command_tag_fails_decode_with_invalid_tag(#[case] tag: u8) {
    assert_eq!(VaultCommand::decode(&[tag]), Err(CodecError::InvalidTag));
}

/// The full epoch command surface round-trips with its exact stable tags
/// (1, 2, 11, and 12-22).
#[rstest]
#[case(VaultCommand::RequestWithdraw { owner: "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF".into(), receiver: "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF".into(), shares: 10, min_assets_out: 1 }, 1u8)]
#[case(VaultCommand::ExecuteWithdraw { caller: "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF".into() }, 2)]
#[case(VaultCommand::AbortWithdrawing { caller: "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF".into(), op_id: 7 }, 11)]
#[case(VaultCommand::ConfigureEpochSettlement { caller: "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF".into(), max_report_age_ns: 3_600_000_000_000 }, 12)]
#[case(VaultCommand::RequestDeposit { owner: "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF".into(), assets: 100, min_shares_out: 1 }, 13)]
#[case(VaultCommand::CancelPendingDeposit { owner: "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF".into(), request_id: 3 }, 14)]
#[case(VaultCommand::BeginEpochCutoff { caller: "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF".into(), cutoff_ns: 1_700_000_000_000_000_000 }, 15)]
#[case(VaultCommand::SettleEpoch { caller: "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF".into() }, 16)]
#[case(VaultCommand::AdmitPendingDeposit { caller: "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF".into(), request_id: 5 }, 17)]
#[case(VaultCommand::CancelPendingWithdrawal { owner: "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF".into(), request_id: 9 }, 18)]
#[case(VaultCommand::GetEpochState, 19)]
#[case(VaultCommand::GetEpochSnapshot { epoch_id: 2 }, 20)]
#[case(VaultCommand::GetCustodialReportMetadata { market_id: 1 }, 21)]
#[case(VaultCommand::SeedEpochSupply { caller: "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF".into(), receiver: "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF".into(), assets: 1_000 }, 22)]
fn epoch_command_round_trips_with_stable_tag(#[case] command: VaultCommand, #[case] tag: u8) {
    let encoded = command.encode();
    assert_eq!(encoded[0], tag);
    assert_eq!(VaultCommand::decode(&encoded), Ok(command));
}
