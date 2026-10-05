//! Exact-conformance smoke tests for the dedicated epoch deploy target.
//!
//! These tests pin, on the shipped thin package itself:
//! 1. its dedicated package/version/capability identity (epoch mask only),
//! 2. that every immediate command tag fails the epoch wire decoder with
//!    `InvalidTag` and is rejected by `execute` with `InvalidInput`,
//! 3. that the full epoch command surface (tags 1, 2, 11, 12-21) round-trips
//!    with stable tags, and
//! 4. that the epoch settlement law (role gates, cutoff closedness, intake
//!    custody, cancellation, and epoch state views) is really served through
//!    the dedicated contract with per-deployment storage.
use rstest::rstest;
use soroban_sdk::{
    contract, contractimpl,
    testutils::{Address as _, Ledger, LedgerInfo},
    token::StellarAssetClient, Address, Bytes, Env, String,
};
use std::string::String as StdString;
use templar_soroban_epoch_runtime::SorobanEpochVaultContract;
use templar_soroban_runtime::error::ContractError;
use templar_soroban_runtime::RUNTIME_VERSION as RUNTIME_CRATE_VERSION;
use templar_soroban_shared_types::{
    BeginEpochCutoffReceipt, CodecError, ConfigureEpochSettlementReceipt, EpochStateViewReceipt,
    ReportMetadataReceipt, VaultCommand, EPOCH_PHASE_CUTOFF, EPOCH_PHASE_OPEN,
    RUNTIME_EPOCH_FEATURE_FLAGS,
};

fn address_text(_env: &Env, address: &Address) -> StdString {
    StdString::from_utf8(address.to_string().to_bytes().to_alloc_vec()).expect("utf-8 strkey")
}

/// Invokes the dedicated epoch target's `execute` entrypoint in contract
/// context, exactly as an on-chain caller would through payload submission.
fn execute_as_contract(env: &Env, contract: &Address, command: &VaultCommand) -> Result<Bytes, ContractError> {
    let payload = command.encode();
    env.as_contract(contract, || {
        SorobanEpochVaultContract::execute(env.clone(), Bytes::from_slice(env, &payload))
    })
}

fn decode_epoch_state_view(bytes: &Bytes) -> EpochStateViewReceipt {
    EpochStateViewReceipt::decode(&bytes.to_alloc_vec()).expect("epoch state view receipt")
}

#[contract]
struct StubCounterpart;

#[contractimpl]
impl StubCounterpart {
    pub fn noop(env: Env) -> u32 {
        let _ = env;
        0
    }
}

struct Fixture {
    env: Env,
    epoch_contract: Address,
    curator: Address,
    governance: Address,
    asset: Address,
    owner: Address,
    attacker: Address,
}

impl Fixture {
    fn new() -> Self {
        let env = Env::default();
        env.mock_all_auths_allowing_non_root_auth();
        env.ledger().set(LedgerInfo {
            protocol_version: 25,
            ..Default::default()
        });
        let epoch_contract = env.register(SorobanEpochVaultContract, ());
        let curator = Address::generate(&env);
        let governance = env.register(StubCounterpart, ());
        let asset_admin = Address::generate(&env);
        let asset = env
            .register_stellar_asset_contract_v2(asset_admin.clone())
            .address();
        let share = env
            .register_stellar_asset_contract_v2(epoch_contract.clone())
            .address();
        let owner = Address::generate(&env);
        let attacker = Address::generate(&env);
        env.as_contract(&epoch_contract, || {
            SorobanEpochVaultContract::initialize(
                env.clone(),
                curator.clone(),
                governance.clone(),
                asset.clone(),
                share,
                0,
                0,
            )
            .expect("fresh epoch deployment initializes");
        });
        Self {
            env,
            epoch_contract,
            curator,
            governance,
            asset,
            owner,
            attacker,
        }
    }

    fn epoch_state(&self) -> EpochStateViewReceipt {
        decode_epoch_state_view(
            &execute_as_contract(
                &self.env,
                &self.epoch_contract,
                &VaultCommand::GetEpochState,
            )
            .expect("epoch state view"),
        )
    }
}

/// The dedicated target reports its own package version and the exact epoch
/// capability mask, and never the default runtime identity.
#[test]
fn version_reports_dedicated_package_and_epoch_mask() {
    let env = Env::default();
    env.mock_all_auths();
    let epoch_contract = env.register(SorobanEpochVaultContract, ());
    let (version, mask) =
        env.as_contract(&epoch_contract, || SorobanEpochVaultContract::version(env.clone()));
    assert_eq!(version, String::from_str(&env, env!("CARGO_PKG_VERSION")));
    assert_ne!(version, String::from_str(&env, RUNTIME_CRATE_VERSION));
    assert_eq!(mask, 0xA0);
    assert_eq!(mask, RUNTIME_EPOCH_FEATURE_FLAGS);
    // No immediate action capability and no companion-upgrade capability.
    assert_eq!(mask & 0x1F, 0);
    assert_eq!(mask & 0x40, 0);
    // The default immediate product masks (current and shipped v1) differ.
    assert_ne!(mask, 0xBF);
    assert_ne!(mask, 0x3F);
}

/// Every immediate-only command tag fails the epoch wire decoder with
/// `InvalidTag`, both as a bare tag byte and as a realistic immediate payload.
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

/// The epoch command surface round-trips with its exact stable tags.
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
fn epoch_command_round_trips_with_stable_tag(#[case] command: VaultCommand, #[case] tag: u8) {
    let encoded = command.encode();
    assert_eq!(encoded[0], tag);
    assert_eq!(VaultCommand::decode(&encoded), Ok(command));
}

/// `execute` on the dedicated target rejects every immediate command payload
/// with `InvalidInput` before any vault state is read.
#[rstest]
#[case(0u8)]
#[case(3)]
#[case(4)]
#[case(5)]
#[case(6)]
#[case(7)]
#[case(8)]
#[case(9)]
#[case(10)]
#[case(255)]
fn execute_rejects_immediate_payloads(#[case] tag: u8) {
    let fix = Fixture::new();
    let err = fix
        .env
        .as_contract(&fix.epoch_contract, || {
            SorobanEpochVaultContract::execute(fix.env.clone(), Bytes::from_slice(&fix.env, &[tag]))
        })
        .expect_err("immediate tag must not execute on the epoch target");
    assert_eq!(err, ContractError::InvalidInput);
    // Rejection happened without advancing the epoch lifecycle at all.
    let view = fix.epoch_state();
    assert_eq!(view.phase, EPOCH_PHASE_OPEN);
    assert_eq!(view.intake_epoch, 1);
}

/// The epoch lifecycle law is served for real through the dedicated target:
/// the epoch state view reflects each transition, cutoff is closed and
/// role-gated, settlement fails closed, and pending-deposit and withdrawal
/// cancellation reach the custody law.
#[test]
fn epoch_lifecycle_law_is_served_through_the_dedicated_target() {
    let fix = Fixture::new();

    // Fresh deployment: intake is open at the first settlement epoch.
    let view = fix.epoch_state();
    assert_eq!(view.phase, EPOCH_PHASE_OPEN);
    assert_eq!(view.intake_epoch, 1);
    assert_eq!(view.cutoff_ns, None);
    assert_eq!(view.last_settled_epoch_id, None);

    // Governance configures the report-freshness law; a stranger may not.
    let err = execute_as_contract(
        &fix.env,
        &fix.epoch_contract,
        &VaultCommand::ConfigureEpochSettlement {
            caller: address_text(&fix.env, &fix.attacker),
            max_report_age_ns: 60_000_000_000,
        },
    )
    .expect_err("epoch settlement configuration is governance-only");
    assert_eq!(err, ContractError::Unauthorized);
    let receipt = execute_as_contract(
        &fix.env,
        &fix.epoch_contract,
        &VaultCommand::ConfigureEpochSettlement {
            caller: address_text(&fix.env, &fix.governance),
            max_report_age_ns: 60_000_000_000,
        },
    )
    .expect("governance may configure epoch settlement");
    assert_eq!(
        ConfigureEpochSettlementReceipt::decode(&receipt.to_alloc_vec())
            .expect("configuration receipt")
            .max_report_age_ns,
        60_000_000_000
    );

    // Without an epoch cutoff, settlement fails closed and reports stay
    // unavailable on this instance.
    let err = execute_as_contract(
        &fix.env,
        &fix.epoch_contract,
        &VaultCommand::SettleEpoch {
            caller: address_text(&fix.env, &fix.curator),
        },
    )
    .expect_err("settlement requires a cutoff");
    assert_eq!(err, ContractError::InvalidState);
    let receipt = execute_as_contract(
        &fix.env,
        &fix.epoch_contract,
        &VaultCommand::GetCustodialReportMetadata { market_id: 0 },
    )
    .expect("custodial report metadata is served");
    assert_eq!(
        ReportMetadataReceipt::decode(&receipt.to_alloc_vec()).expect("report metadata receipt"),
        ReportMetadataReceipt::Unavailable { market_id: 0 }
    );
    let err = execute_as_contract(
        &fix.env,
        &fix.epoch_contract,
        &VaultCommand::GetEpochSnapshot { epoch_id: 1 },
    )
    .expect_err("no snapshot exists before settlement");
    assert_eq!(err, ContractError::EpochSnapshotUnavailable);

    // Cutoff is role-gated: an unauthorized caller cannot close intake,
    // and the refused attempt leaves the view open.
    let cutoff_ns = 10_000_000_000u64;
    fix.env.ledger().set(LedgerInfo {
        timestamp: cutoff_ns / 1_000_000_000,
        protocol_version: 25,
        ..Default::default()
    });
    let err = execute_as_contract(
        &fix.env,
        &fix.epoch_contract,
        &VaultCommand::BeginEpochCutoff {
            caller: address_text(&fix.env, &fix.attacker),
            cutoff_ns,
        },
    )
    .expect_err("cutoff is allocator-only");
    assert_eq!(err, ContractError::Unauthorized);
    assert_eq!(fix.epoch_state().phase, EPOCH_PHASE_OPEN);

    // The authorized curator closes intake for the settling epoch.
    let receipt = execute_as_contract(
        &fix.env,
        &fix.epoch_contract,
        &VaultCommand::BeginEpochCutoff {
            caller: address_text(&fix.env, &fix.curator),
            cutoff_ns,
        },
    )
    .expect("authorized cutoff closes intake");
    assert_eq!(
        BeginEpochCutoffReceipt::decode(&receipt.to_alloc_vec()).expect("cutoff receipt"),
        BeginEpochCutoffReceipt {
            epoch_id: 1,
            cutoff_ns,
        }
    );
    let view = fix.epoch_state();
    assert_eq!(view.phase, EPOCH_PHASE_CUTOFF);
    assert_eq!(view.cutoff_ns, Some(cutoff_ns));
    assert_eq!(view.last_settled_epoch_id, None);

    // Settlement stays closed: no adapter report has been accepted, so the
    // epoch does not settle and nothing is admitted.
    match execute_as_contract(
        &fix.env,
        &fix.epoch_contract,
        &VaultCommand::SettleEpoch {
            caller: address_text(&fix.env, &fix.curator),
        },
    ) {
        Ok(_) => panic!("unexpected settlement without accepted reports"),
        Err(err) => assert_ne!(
            err,
            ContractError::Unauthorized,
            "an authorized caller must reach settlement law, not the role gate"
        ),
    }
    let view = fix.epoch_state();
    assert_eq!(view.phase, EPOCH_PHASE_CUTOFF);
    assert_eq!(view.last_settled_epoch_id, None);
    let err = execute_as_contract(
        &fix.env,
        &fix.epoch_contract,
        &VaultCommand::AdmitPendingDeposit {
            caller: address_text(&fix.env, &fix.curator),
            request_id: 1,
        },
    )
    .expect_err("nothing can be admitted without settlement");
    assert_ne!(err, ContractError::Unauthorized);

    // Cancellation law is reached through the queue: unknown requests fail
    // against the custody records of this instance.
    let err = execute_as_contract(
        &fix.env,
        &fix.epoch_contract,
        &VaultCommand::CancelPendingDeposit {
            owner: address_text(&fix.env, &fix.owner),
            request_id: 1,
        },
    )
    .expect_err("no pending deposit custody exists on this instance");
    assert_eq!(err, ContractError::StorageError);
    let err = execute_as_contract(
        &fix.env,
        &fix.epoch_contract,
        &VaultCommand::CancelPendingWithdrawal {
            owner: address_text(&fix.env, &fix.owner),
            request_id: 1,
        },
    )
    .expect_err("no pending withdrawal escrow exists on this instance");
    assert_ne!(err, ContractError::Unauthorized);

    // Pending-deposit intake custody is enforced: intake is closed at the
    // cutoff, so intake attempts fail at the custody law.
    StellarAssetClient::new(&fix.env, &fix.asset).mint(&fix.owner, &10_000);
    let err = execute_as_contract(
        &fix.env,
        &fix.epoch_contract,
        &VaultCommand::RequestDeposit {
            owner: address_text(&fix.env, &fix.owner),
            assets: 1_000,
            min_shares_out: 0,
        },
    )
    .expect_err("intake is closed at cutoff");
    assert_eq!(err, ContractError::InvalidState);
}

/// A second deployment of the same target has its own storage and is
/// unaffected by the first deployment's cutoff or configuration.
#[test]
fn deployments_do_not_share_epoch_state() {
    let fix = Fixture::new();
    let cutoff_ns = 10_000_000_000u64;
    execute_as_contract(
        &fix.env,
        &fix.epoch_contract,
        &VaultCommand::ConfigureEpochSettlement {
            caller: address_text(&fix.env, &fix.governance),
            max_report_age_ns: 60_000_000_000,
        },
    )
    .expect("governance configures epoch settlement");
    fix.env.ledger().set(LedgerInfo {
        timestamp: cutoff_ns / 1_000_000_000,
        protocol_version: 25,
        ..Default::default()
    });
    execute_as_contract(
        &fix.env,
        &fix.epoch_contract,
        &VaultCommand::BeginEpochCutoff {
            caller: address_text(&fix.env, &fix.curator),
            cutoff_ns,
        },
    )
    .expect("authorized cutoff closes intake");
    assert_eq!(fix.epoch_state().phase, EPOCH_PHASE_CUTOFF);

    let env2 = Env::default();
    env2.mock_all_auths_allowing_non_root_auth();
    env2.ledger().set(LedgerInfo {
        protocol_version: 25,
        ..Default::default()
    });
    let epoch_contract2 = env2.register(SorobanEpochVaultContract, ());
    let governance2 = env2.register(StubCounterpart, ());
    let asset2 = env2
        .register_stellar_asset_contract_v2(Address::generate(&env2))
        .address();
    let share2 = env2
        .register_stellar_asset_contract_v2(epoch_contract2.clone())
        .address();
    env2.as_contract(&epoch_contract2, || {
        SorobanEpochVaultContract::initialize(
            env2.clone(),
            Address::generate(&env2),
            governance2,
            asset2,
            share2,
            0,
            0,
        )
        .expect("second deployment initializes");
    });
    let fresh = decode_epoch_state_view(
        &execute_as_contract(&env2, &epoch_contract2, &VaultCommand::GetEpochState)
            .expect("epoch state view"),
    );
    assert_eq!(fresh.phase, EPOCH_PHASE_OPEN);
    assert_eq!(fresh.intake_epoch, 1);
    assert_eq!(fresh.cutoff_ns, None);
}
