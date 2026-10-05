//! Behavior tests for the custodial valuation report intake.
//!
//! Only the configured custodian can create a settlement-eligible
//! valuation. Rejections are asserted through public read-only views so
//! adapter storage never advances on a rejected submission.

use super::*;
use soroban_sdk::testutils::{Address as _, Ledger as _};
use soroban_sdk::{contract, contractimpl};
use templar_soroban_shared_types::CustodialValuationView;

#[contract]
struct DummyContract;

#[contractimpl]
impl DummyContract {}

fn setup(env: &Env) -> (Address, Address, Address, Address, Address) {
    let admin = Address::generate(env);
    let vault = env.register(DummyContract, ());
    let custodian = Address::generate(env);
    let asset = env
        .register_stellar_asset_contract_v2(Address::generate(env))
        .address();
    let adapter = env.register(CustodialAdapterContract, (&admin, &vault, &custodian, &asset));
    (adapter, admin, vault, custodian, asset)
}

fn report(
    env: &Env,
    adapter: &Address,
    vault: &Address,
    asset: &Address,
    sequence: u64,
    as_of: u64,
    assets_value: i128,
) -> CustodialValuationReport {
    CustodialValuationReport {
        vault: vault.clone(),
        adapter: adapter.clone(),
        asset: asset.clone(),
        network_id: env.ledger().network_id(),
        sequence,
        as_of,
        assets_value,
        report_hash: None,
    }
}

fn client<'a>(env: &'a Env, adapter: &Address) -> CustodialAdapterContractClient<'a> {
    CustodialAdapterContractClient::new(env, adapter)
}

fn valuation(env: &Env, adapter: &Address, asset: &Address) -> Option<CustodialValuationView> {
    env.as_contract(adapter, || {
        CustodialAdapterContract::valuation(env.clone(), asset.clone())
    })
    .unwrap()
}

fn total_assets(env: &Env, adapter: &Address, asset: &Address) -> i128 {
    env.as_contract(adapter, || {
        CustodialAdapterContract::total_assets(env.clone(), asset.clone())
    })
}

fn reported_at(env: &Env, adapter: &Address, asset: &Address) -> Option<u64> {
    env.as_contract(adapter, || {
        CustodialAdapterContract::reported_at(env.clone(), asset.clone())
    })
    .unwrap()
}

fn submit(
    env: &Env,
    adapter: &Address,
    caller: &Address,
    payload: &CustodialValuationReport,
) -> Result<(), AdapterError> {
    env.as_contract(adapter, || {
        CustodialAdapterContract::submit_report(env.clone(), caller.clone(), payload.clone())
    })
}

fn advance(env: &Env, adapter: &Address, caller: &Address, payload: &CustodialValuationReport) {
    assert_eq!(submit(env, adapter, caller, payload), Ok(()));
}

fn snapshot(env: &Env, adapter: &Address, asset: &Address) -> (i128, Option<u64>, u64, Option<CustodialValuationView>, bool) {
    (
        total_assets(env, adapter, asset),
        reported_at(env, adapter, asset),
        env.as_contract(adapter, || {
            CustodialAdapterContract::report_nonce(env.clone(), asset.clone())
        })
        .unwrap(),
        valuation(env, adapter, asset),
        client(env, adapter).paused(),
    )
}

fn reject(
    env: &Env,
    adapter: &Address,
    caller: &Address,
    asset: &Address,
    payload: &CustodialValuationReport,
    expected: AdapterError,
    before: &(i128, Option<u64>, u64, Option<CustodialValuationView>, bool),
) {
    assert_eq!(submit(env, adapter, caller, payload), Err(expected));
    assert_eq!(snapshot(env, adapter, asset), *before);
}

#[test]
fn only_custodian_reports_are_accepted_and_stay_settlement_eligible() {
    let env = Env::default();
    env.mock_all_auths();
    let (adapter, admin, vault, custodian, asset) = setup(&env);
    env.ledger().set_timestamp(1_000);
    let before = snapshot(&env, &adapter, &asset);

    let hash = BytesN::from_array(&env, &[7u8; 32]);
    let mut accepted = report(&env, &adapter, &vault, &asset, 1, 900, 500);
    accepted.report_hash = Some(hash.clone());
    advance(&env, &adapter, &custodian, &accepted);
    assert_eq!(
        valuation(&env, &adapter, &asset),
        Some((1, 900, 1_000, 500, Some(hash.clone())))
    );
    assert_eq!(total_assets(&env, &adapter, &asset), 0);
    assert_eq!(reported_at(&env, &adapter, &asset), None);

    let replay = report(&env, &adapter, &vault, &asset, 1, 1_000, 500);
    for caller in [&admin, &vault, &Address::generate(&env)] {
        assert_eq!(
            submit(&env, &adapter, caller, &replay),
            Err(AdapterError::Unauthorized)
        );
    }
    assert_eq!(
        valuation(&env, &adapter, &asset),
        Some((1, 900, 1_000, 500, Some(hash)))
    );
    assert_eq!(total_assets(&env, &adapter, &asset), 0);
    assert_eq!(reported_at(&env, &adapter, &asset), None);
    assert_eq!(snapshot(&env, &adapter, &asset).0, before.0);
}

#[test]
fn domain_binding_payload_and_timing_rejections_never_advance_state() {
    let env = Env::default();
    env.mock_all_auths();
    let (adapter, _admin, vault, custodian, asset) = setup(&env);
    env.ledger().set_timestamp(1_000);
    let before = snapshot(&env, &adapter, &asset);

    let foreign_vault = env.register(DummyContract, ());
    let foreign_asset = env
        .register_stellar_asset_contract_v2(Address::generate(&env))
        .address();
    let mut wrong_vault = report(&env, &adapter, &vault, &asset, 1, 900, 500);
    wrong_vault.vault = foreign_vault.clone();
    let mut wrong_asset = report(&env, &adapter, &vault, &asset, 1, 900, 500);
    wrong_asset.asset = foreign_asset.clone();
    let mut wrong_network = report(&env, &adapter, &vault, &asset, 1, 900, 500);
    wrong_network.network_id = BytesN::from_array(&env, &[9u8; 32]);

    let cases = [
        (&wrong_vault, AdapterError::ReportDomainMismatch),
        (&wrong_asset, AdapterError::ReportDomainMismatch),
        (&wrong_network, AdapterError::ReportDomainMismatch),
        (
            &report(&env, &foreign_asset, &vault, &asset, 1, 900, 500),
            AdapterError::ReportDomainMismatch,
        ),
        (
            &report(&env, &adapter, &vault, &asset, 0, 900, 500),
            AdapterError::ReportPayloadInvalid,
        ),
        (
            &report(&env, &adapter, &vault, &asset, 1, 0, 500),
            AdapterError::ReportPayloadInvalid,
        ),
        (
            &report(&env, &adapter, &vault, &asset, 1, 900, -1),
            AdapterError::ReportPayloadInvalid,
        ),
        (
            &report(&env, &adapter, &vault, &asset, 1, 1_001, 500),
            AdapterError::ReportValuationFuture,
        ),
    ];
    for (payload, expected) in cases {
        reject(&env, &adapter, &custodian, &asset, payload, expected, &before);
    }
}

#[test]
fn sequence_replays_gaps_and_exhaustion_are_rejected() {
    let env = Env::default();
    env.mock_all_auths();
    let (adapter, _admin, vault, custodian, asset) = setup(&env);
    env.ledger().set_timestamp(1_000);
    let before = snapshot(&env, &adapter, &asset);

    for (sequence, as_of) in [
        (0u64, 900u64),
        (2, 900),
        (u64::MAX, 900),
        (u64::MAX, 1_000),
    ] {
        reject(
            &env,
            &adapter,
            &custodian,
            &asset,
            &report(&env, &adapter, &vault, &asset, sequence, as_of, 500),
            if sequence == 0 {
                AdapterError::ReportPayloadInvalid
            } else {
                AdapterError::ReportSequenceInvalid
            },
            &before,
        );
    }

    let first = report(&env, &adapter, &vault, &asset, 1, 900, 500);
    advance(&env, &adapter, &custodian, &first);
    for (sequence, as_of) in [(1u64, 1_000u64), (3, 1_000), (u64::MAX, 1_000)] {
        reject(
            &env,
            &adapter,
            &custodian,
            &asset,
            &report(&env, &adapter, &vault, &asset, sequence, as_of, 500),
            AdapterError::ReportSequenceInvalid,
            &(0, None, 0, Some((1, 900, 1_000, 500, None)), false),
        );
    }

    env.as_contract(&adapter, || {
        store_accepted_sequence(&env, &asset, u64::MAX);
    });
    let exhausted_before = snapshot(&env, &adapter, &asset);
    reject(
        &env,
        &adapter,
        &custodian,
        &asset,
        &report(&env, &adapter, &vault, &asset, 1, 901, 500),
        AdapterError::ReportSequenceExhausted,
        &exhausted_before,
    );

    env.as_contract(&adapter, || {
        env.storage().instance().remove(&DataKey::AcceptedSequence(asset.clone()));
    });
    env.ledger().set_timestamp(1_001);
    advance(
        &env,
        &adapter,
        &custodian,
        &report(&env, &adapter, &vault, &asset, 1, 1_001, 500),
    );
    assert_eq!(
        valuation(&env, &adapter, &asset),
        Some((1, 1_001, 1_001, 500, None))
    );
}

#[test]
fn non_monotonic_valuation_times_are_rejected() {
    let env = Env::default();
    env.mock_all_auths();
    let (adapter, _admin, vault, custodian, asset) = setup(&env);
    env.ledger().set_timestamp(1_000);

    advance(
        &env,
        &adapter,
        &custodian,
        &report(&env, &adapter, &vault, &asset, 1, 900, 500),
    );
    let after_first = snapshot(&env, &adapter, &asset);
    for as_of in [900u64, 800u64] {
        reject(
            &env,
            &adapter,
            &custodian,
            &asset,
            &report(&env, &adapter, &vault, &asset, 2, as_of, 500),
            AdapterError::ReportAsOfNonMonotonic,
            &after_first,
        );
    }
}

#[test]
fn paused_blocks_report_submission_and_pause_roles_are_enforced() {
    let env = Env::default();
    env.mock_all_auths();
    let (adapter, admin, vault, custodian, asset) = setup(&env);
    env.ledger().set_timestamp(1_000);
    advance(
        &env,
        &adapter,
        &custodian,
        &report(&env, &adapter, &vault, &asset, 1, 900, 500),
    );
    let before = snapshot(&env, &adapter, &asset);

    assert_eq!(
        env.as_contract(&adapter, || {
            CustodialAdapterContract::set_paused(env.clone(), Address::generate(&env), true)
        }),
        Err(AdapterError::Unauthorized)
    );
    assert_eq!(snapshot(&env, &adapter, &asset), before);
    env.as_contract(&adapter, || {
        CustodialAdapterContract::set_paused(env.clone(), admin.clone(), true)
    })
    .unwrap();
    let paused = snapshot(&env, &adapter, &asset);
    let rejected = report(&env, &adapter, &vault, &asset, 2, 1_000, 500);
    for caller in [&custodian, &admin, &vault] {
        reject(&env, &adapter, caller, &asset, &rejected, AdapterError::Paused, &paused);
    }

    client(&env, &adapter).set_paused(&vault, &false);
    advance(
        &env,
        &adapter,
        &custodian,
        &report(&env, &adapter, &vault, &asset, 2, 1_000, 500),
    );
    assert_eq!(
        valuation(&env, &adapter, &asset),
        Some((2, 1_000, 1_000, 500, None))
    );
    assert!(!before.4);
}

#[test]
fn legacy_and_custody_mutations_invalidate_settlement_eligibility() {
    let env = Env::default();
    env.mock_all_auths();
    let (adapter, admin, vault, custodian, asset) = setup(&env);
    env.ledger().set_timestamp(1_000);
    advance(
        &env,
        &adapter,
        &custodian,
        &report(&env, &adapter, &vault, &asset, 1, 900, 500),
    );
    let eligible = snapshot(&env, &adapter, &asset);
    assert_eq!(eligible.3, Some((1, 900, 1_000, 500, None)));

    client(&env, &adapter).set_reported_assets(&admin, &asset, &0, &700, &1);
    let after_legacy = snapshot(&env, &adapter, &asset);
    assert_eq!(after_legacy.3, None);
    assert_eq!(after_legacy.0, 700);
    assert_eq!(after_legacy.1, Some(1_000));
    assert_eq!(after_legacy.2, 1);
    let first = report(&env, &adapter, &vault, &asset, 1, 900, 500);
    reject(
        &env,
        &adapter,
        &custodian,
        &asset,
        &first,
        AdapterError::ReportSequenceInvalid,
        &after_legacy,
    );
    env.ledger().set_timestamp(1_001);
    advance(
        &env,
        &adapter,
        &custodian,
        &report(&env, &adapter, &vault, &asset, 2, 1_001, 700),
    );
    let after_reaccept = snapshot(&env, &adapter, &asset);
    assert_eq!(after_reaccept.3, Some((2, 1_001, 1_001, 700, None)));

    let asset_admin = soroban_sdk::token::StellarAssetClient::new(&env, &asset);
    asset_admin.mint(&adapter, &300);
    client(&env, &adapter).supply(&vault, &asset, &100);
    let after_supply = snapshot(&env, &adapter, &asset);
    assert_eq!(after_supply.3, None);
    assert_eq!(after_supply.0, 800);
    reject(
        &env,
        &adapter,
        &custodian,
        &asset,
        &report(&env, &adapter, &vault, &asset, 2, 1_001, 700),
        AdapterError::ReportSequenceInvalid,
        &after_supply,
    );

    client(&env, &adapter).withdraw(&vault, &asset, &40);
    let after_withdraw = snapshot(&env, &adapter, &asset);
    assert_eq!(after_withdraw.3, None);
    assert_eq!(after_withdraw.0, 760);
    env.ledger().set_timestamp(1_002);
    advance(
        &env,
        &adapter,
        &custodian,
        &report(&env, &adapter, &vault, &asset, 3, 1_002, 760),
    );
    asset_admin.mint(&custodian, &50);
    soroban_sdk::token::Client::new(&env, &asset)
        .mock_all_auths()
        .transfer(&custodian, &adapter, &50);
    assert_eq!(
        client(&env, &adapter).progress_withdrawal(&vault, &asset, &50),
        50
    );
    let after_progress = snapshot(&env, &adapter, &asset);
    assert_eq!(after_progress.3, None);
    assert_eq!(after_progress.0, 710);
    env.ledger().set_timestamp(1_003);
    advance(
        &env,
        &adapter,
        &custodian,
        &report(&env, &adapter, &vault, &asset, 4, 1_003, 710),
    );
    assert_eq!(
        valuation(&env, &adapter, &asset),
        Some((4, 1_003, 1_003, 710, None))
    );
}
