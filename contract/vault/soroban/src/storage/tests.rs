//! Storage-layer law regression tests.
//!
//! Every test here is a discriminating check on the storage law itself:
//! persisted deposit share floors, owner-bound removal, at-most-once
//! admission consumption, fail-closed corruption handling, legacy
//! quote-dropping migration, governance-gated epoch law, and accepted-report
//! replay prevention. The dummy contract below exists only to give the
//! storage layer a contract context for persistent storage; none of its
//! methods are used.

use super::deposit::{
    AggregateKey, DepositPageKey, NextIdKey, PendingDepositAggregate, PendingDepositRecord,
    PendingStorage, encode_pending_deposit_aggregate, encode_pending_deposit_page,
};
use super::epoch::{
    AcceptedReportRecord, decode_accepted_report_record, decode_epoch_record, decode_u64_record,
    encode_accepted_report_record, encode_epoch_record, encode_u64_record,
};
use super::{
    StorageKind, SorobanStorage, encode_withdraw_queue_page, epoch_state_from_state_header_blob,
};
use alloc::vec;
use alloc::vec::Vec;
use soroban_sdk::{Env, contract, contractimpl, testutils::Address as _};
use templar_vault_kernel::{
    Address, EpochId, EpochPhase, EpochState, PendingDeposit, PendingWithdrawal, TimestampNs,
    ValuationReportRef,
};

use super::{decode_markets, encode_markets};
use soroban_sdk::Map;
use templar_curator_primitives::policy::state::{MarketConfig, OrderedMap};

#[contract]
struct StorageTestContract;

#[contractimpl]
impl StorageTestContract {
    pub fn noop(_env: Env) {}
}

fn contract_id(env: &Env) -> soroban_sdk::Address {
    env.register(StorageTestContract, ())
}

#[must_use]
fn test_address(byte: u8) -> Address {
    Address([byte; 32])
}

#[must_use]
fn ts(seconds: u64) -> TimestampNs {
    TimestampNs::from_nanos(seconds.saturating_mul(1_000_000_000))
}

#[must_use]
fn record(
    owner: Address,
    assets: u128,
    floor: u128,
    seconds: u64,
    epoch: u64,
) -> PendingDepositRecord {
    PendingDepositRecord {
        owner,
        assets,
        min_shares_out: floor,
        requested_at_ns: ts(seconds),
        epoch_id: EpochId::new(epoch),
    }
}

#[must_use]
fn live_record() -> PendingDepositRecord {
    record(test_address(7), 1_000, 77, 1_700_000_000, 1)
}

#[must_use]
fn absent_record() -> PendingDepositRecord {
    record(Address([0u8; 32]), 0, 0, 0, 0)
}

#[must_use]
fn empty_slots() -> [(u64, PendingDepositRecord); 32] {
    // Exactly one page: the deposit page codec validates its own slot
    // count, so any drift between this fixture and storage law fails
    // closed inside `encode_pending_deposit_page`.
    core::array::from_fn(|_| (0u64, absent_record()))
}

#[must_use]
fn deposit(assets: u128, seconds: u64, epoch: u64) -> PendingDeposit {
    PendingDeposit::new(test_address(7), assets, ts(seconds), EpochId::new(epoch))
        .expect("lawful deposit")
}

#[must_use]
fn epoch_state(
    phase: EpochPhase,
    intake_epoch: u64,
    cutoff_seconds: Option<u64>,
) -> EpochState {
    EpochState {
        phase,
        intake_epoch: EpochId::new(intake_epoch),
        cutoff_ns: cutoff_seconds.map(ts),
        last_settled: None,
    }
}

#[must_use]
fn genesis() -> EpochState {
    epoch_state(EpochPhase::Open, 1, None)
}

#[must_use]
fn settlement_state(cutoff_seconds: u64, snapshot_seconds: Option<u64>) -> EpochState {
    let cutoff = ts(cutoff_seconds);
    let snapshot = snapshot_seconds.map(|seconds| {
        templar_vault_kernel::EpochSnapshot::bind(
            EpochId::new(1),
            cutoff,
            &report(1, seconds),
            1_000_000_000,
            1_000,
        )
        .expect("lawful snapshot")
    });
    EpochState {
        phase: EpochPhase::Open,
        intake_epoch: EpochId::new(2),
        cutoff_ns: None,
        last_settled: snapshot,
    }
}

#[must_use]
fn report(seq: u64, seconds: u64) -> ValuationReportRef {
    ValuationReportRef {
        report_seq: seq,
        as_of_ns: ts(seconds),
        report_hash: [seq as u8; 32],
    }
}

#[test]
fn deposit_page_roundtrip_preserves_owner_floor_and_liability() {
    let mut slots = empty_slots();
    slots[0] = (1, live_record());
    slots[5] = (
        6,
        record(test_address(8), 2_500, 5, 1_700_000_001, 1),
    );
    let bytes = encode_pending_deposit_page(0, &slots).expect("encode page");
    let (page, decoded) = super::deposit::decode_pending_deposit_page(&bytes)
        .expect("decode page");
    assert_eq!(page, 0);
    assert_eq!(decoded.len(), slots.len());
    assert_eq!(decoded, slots);
    assert_eq!(decoded[0].1.min_shares_out, 77);
    assert_eq!(decoded[5].1.owner, test_address(8));
    assert_eq!(decoded[5].1.min_shares_out, 5);
    assert_eq!(decoded[1], (0, absent_record()));
}

#[test]
fn deposit_page_rejects_corruption() {
    let mut slots = empty_slots();
    slots[0] = (1, live_record());
    let good = encode_pending_deposit_page(0, &slots).expect("encode page");

    let mut magic = good.clone();
    magic[0] ^= 0xff;
    assert!(super::deposit::decode_pending_deposit_page(&magic).is_err());

    let mut page = good.clone();
    page[5] ^= 0x01;
    assert!(super::deposit::decode_pending_deposit_page(&page).is_err());

    let mut count = good.clone();
    count[9] ^= 0x01;
    assert!(super::deposit::decode_pending_deposit_page(&count).is_err());

    let mut epoch = good.clone();
    epoch[93] ^= 0x01;
    assert!(super::deposit::decode_pending_deposit_page(&epoch).is_err());

    let mut short = good.clone();
    short.pop();
    assert!(super::deposit::decode_pending_deposit_page(&short).is_err());

    let mut long = good.clone();
    long.push(0);
    assert!(super::deposit::decode_pending_deposit_page(&long).is_err());
}

#[test]
fn deposit_aggregate_roundtrip_and_epoch_validation() {
    let zero = PendingDepositAggregate {
        count: 0,
        total_assets: 0,
        oldest_epoch: None,
    };
    let bytes = encode_pending_deposit_aggregate(&zero);
    assert_eq!(
        super::deposit::decode_pending_deposit_aggregate(&bytes).expect("decode aggregate"),
        zero
    );

    let live = PendingDepositAggregate {
        count: 3,
        total_assets: 6_500,
        oldest_epoch: Some(EpochId::new(1)),
    };
    let live_bytes = encode_pending_deposit_aggregate(&live);
    assert_eq!(
        super::deposit::decode_pending_deposit_aggregate(&live_bytes).expect("decode live"),
        live
    );

    let mut tampered = live_bytes.clone();
    tampered[29] ^= 0x01;
    assert!(super::deposit::decode_pending_deposit_aggregate(&tampered).is_err());

    let orphan = PendingDepositAggregate {
        count: 2,
        total_assets: 3,
        oldest_epoch: Some(EpochId::new(0)),
    };
    assert!(
        super::deposit::decode_pending_deposit_aggregate(&encode_pending_deposit_aggregate(&orphan))
            .is_err()
    );

    let zero_count = PendingDepositAggregate {
        count: 0,
        total_assets: 0,
        oldest_epoch: Some(EpochId::new(1)),
    };
    assert!(
        super::deposit::decode_pending_deposit_aggregate(&encode_pending_deposit_aggregate(
            &zero_count
        ))
        .is_err()
    );

    let mut short = live_bytes.clone();
    short.pop();
    assert!(super::deposit::decode_pending_deposit_aggregate(&short).is_err());
    let mut long = live_bytes.clone();
    long.push(0);
    assert!(super::deposit::decode_pending_deposit_aggregate(&long).is_err());
}

#[test]
fn ledger_aggregates_liability_and_enforce_owner_and_floor_law() {
    let owner_a = test_address(7);
    let owner_b = test_address(8);
    let deposit_a = deposit(1_000, 1_700_000_000, 1);
    let deposit_b = PendingDeposit::new(
        owner_b,
        2_500,
        ts(1_700_000_001),
        EpochId::new(1),
    )
    .expect("lawful deposit");
    let deposit_c = PendingDeposit::new(
        owner_a,
        3_000,
        ts(1_700_000_002),
        EpochId::new(1),
    )
    .expect("lawful deposit");

    let env = Env::default();
    env.mock_all_auths();
    let id = contract_id(&env);
    env.as_contract(&id, || {
        let storage = SorobanStorage::new(&env);
        storage
            .save_epoch_state(&genesis())
            .expect("epoch law stored");
        storage
            .save_max_report_age_ns(60_000_000_000)
            .expect("epoch mode activated");

        let id_a = storage
            .create_pending_deposit(&deposit_a, 77)
            .expect("create first");
        let id_b = storage
            .create_pending_deposit(&deposit_b, 5)
            .expect("create second");
        let id_c = storage
            .create_pending_deposit(&deposit_c, 0)
            .expect("create third");
        assert_eq!((id_a, id_b, id_c), (1, 2, 3));
        assert_eq!(
            storage.next_deposit_request_id().expect("next id"),
            4
        );

        // Liability aggregates reflect the whole ledger, never a page view.
        let stats = storage.pending_deposit_stats().expect("stats");
        assert_eq!(stats.count, 3);
        assert_eq!(stats.total_assets, 6_500);
        assert_eq!(
            storage
                .pending_deposit_oldest_epoch()
                .expect("oldest epoch"),
            Some(EpochId::new(1))
        );
        assert!(
            storage
                .any_pending_deposit_before_epoch(EpochId::new(2))
                .expect("oldest epoch check")
        );
        assert!(
            !storage
                .any_pending_deposit_before_epoch(EpochId::new(1))
                .expect("oldest epoch check")
        );
        storage
            .verify_pending_deposit_integrity()
            .expect("ledger consistent");

        // Owner lookup and the persisted share floors.
        assert_eq!(
            storage
                .pending_deposit_ids_of_owner(&owner_a)
                .expect("owner lookup"),
            vec![1u64, 3]
        );
        assert_eq!(
            storage
                .pending_deposit_ids_of_owner(&owner_b)
                .expect("owner lookup"),
            vec![2u64]
        );
        assert_eq!(
            storage.load_pending_deposit_min_shares_out(id_a).expect("floor"),
            Some(77)
        );
        assert_eq!(
            storage.load_pending_deposit_min_shares_out(id_b).expect("floor"),
            Some(5)
        );
        assert_eq!(
            storage.load_pending_deposit_min_shares_out(id_c).expect("floor"),
            Some(0)
        );
        assert_eq!(
            storage
                .load_pending_deposit_min_shares_out(99)
                .expect("floor lookup"),
            None
        );

        // The persisted floor is immutable: an identical write is a no-op,
        // a replacement fails closed, and the value never changes.
        storage
            .save_pending_deposit_min_shares_out(id_a, 77)
            .expect("idempotent write accepted");
        assert!(storage.save_pending_deposit_min_shares_out(id_a, 78).is_err());
        assert_eq!(
            storage.load_pending_deposit_min_shares_out(id_a).expect("floor"),
            Some(77)
        );
        assert!(storage.save_pending_deposit_min_shares_out(99, 77).is_err());

        // Cancellation is owner-bound and cannot mutate ledger state.
        assert!(
            storage
                .cancel_pending_deposit(&owner_b, id_a)
                .is_err()
        );
        assert_eq!(
            storage
                .load_pending_deposit(id_a)
                .expect("owner read")
                .expect("record present")
                .owner,
            owner_a
        );
        assert_eq!(
            storage.pending_deposit_stats().expect("stats").count,
            3
        );

        storage
            .cancel_pending_deposit(&owner_a, id_a)
            .expect("owner cancel");
        assert_eq!(storage.load_pending_deposit(id_a).expect("owner read"), None);
        assert_eq!(
            storage
                .load_pending_deposit_min_shares_out(id_a)
                .expect("floor lookup"),
            None
        );
        assert_eq!(
            storage.pending_deposit_stats().expect("stats").count,
            2
        );
        assert_eq!(
            storage
                .pending_deposit_ids_of_owner(&owner_a)
                .expect("owner lookup"),
            vec![3u64]
        );
        assert!(
            storage
                .cancel_pending_deposit(&owner_a, id_a)
                .is_err()
        );

        // Admission consumption is atomic, returns the exact persisted
        // record, and is unrepeatable: record presence is the at-most-once
        // authority.
        let taken = storage.take_pending_deposit(id_b).expect("admission take");
        assert_eq!(taken.owner, owner_b);
        assert_eq!(taken.assets, 2_500);
        assert_eq!(taken.min_shares_out, 5);
        assert_eq!(taken.epoch_id, EpochId::new(1));
        assert_eq!(storage.load_pending_deposit(id_b).expect("owner read"), None);
        assert_eq!(
            storage
                .load_pending_deposit_min_shares_out(id_b)
                .expect("floor lookup"),
            None
        );
        assert!(storage.take_pending_deposit(id_b).is_err());
        assert_eq!(
            storage.pending_deposit_stats().expect("stats").total_assets,
            3_000
        );
        assert_eq!(
            storage
                .pending_deposit_oldest_epoch()
                .expect("oldest epoch"),
            Some(EpochId::new(1))
        );

        // Ids are never reused: the next id exceeds every id ever issued.
        let id_d = storage
            .create_pending_deposit(&deposit_c, 0)
            .expect("create after removal");
        assert!(id_d > id_c);

        // A withdrawal floor stored against a cancelled request is gone with
        // the record and can never be re-attached.
        assert!(storage.save_pending_deposit_min_shares_out(id_a, 77).is_err());
        assert!(storage.cancel_pending_deposit(&owner_a, 12_345).is_err());
    });
}

#[test]
fn ledger_requires_epoch_law_and_fails_closed_on_corruption() {
    let future = deposit(1_000, 1_700_000_000, 2);
    let lawful = deposit(1_000, 1_700_000_000, 1);

    // No epoch lifecycle record at all: intake is impossible without the
    // intake-epoch law, and epoch mode is off until governance acts.
    let env = Env::default();
    env.mock_all_auths();
    let id = contract_id(&env);
    env.as_contract(&id, || {
        let storage = SorobanStorage::new(&env);
        assert!(storage.create_pending_deposit(&lawful, 77).is_err());
    });

    // Wrong phase refuses intake: the epoch is closed for new requests.
    let env = Env::default();
    env.mock_all_auths();
    let id = contract_id(&env);
    env.as_contract(&id, || {
        let storage = SorobanStorage::new(&env);
        storage
            .save_epoch_state(&epoch_state(EpochPhase::Cutoff, 1, Some(1_700_000_000)))
            .expect("epoch law stored");
        storage
            .save_max_report_age_ns(60_000_000_000)
            .expect("epoch mode activated");
        assert!(storage.create_pending_deposit(&lawful, 77).is_err());
    });

    // A deposit bound to the wrong epoch is refused even while intake is
    // open.
    let env = Env::default();
    env.mock_all_auths();
    let id = contract_id(&env);
    env.as_contract(&id, || {
        let storage = SorobanStorage::new(&env);
        storage
            .save_epoch_state(&genesis())
            .expect("epoch law stored");
        storage
            .save_max_report_age_ns(60_000_000_000)
            .expect("epoch mode activated");
        assert!(storage.create_pending_deposit(&future, 77).is_err());
    });

    // A live ledger with a rewritten next-id watermark fails closed: ids
    // are never re-issued and aggregates never trust a stale watermark.
    let env = Env::default();
    env.mock_all_auths();
    let id = contract_id(&env);
    env.as_contract(&id, || {
        let storage = SorobanStorage::new(&env);
        storage
            .save_epoch_state(&genesis())
            .expect("epoch law stored");
        storage
            .save_max_report_age_ns(60_000_000_000)
            .expect("epoch mode activated");
        storage
            .create_pending_deposit(&lawful, 77)
            .expect("create deposit");
        env.storage().persistent().set(&NextIdKey, &0u64);
        assert!(storage.pending_deposit_stats().is_err());
        assert!(storage.verify_pending_deposit_integrity().is_err());
        assert!(storage.create_pending_deposit(&lawful, 77).is_err());
    });

    // A tampered page fails every aggregate read: liability recomputes
    // from pages and never trusts a stored figure.
    let env = Env::default();
    env.mock_all_auths();
    let id = contract_id(&env);
    env.as_contract(&id, || {
        let storage = SorobanStorage::new(&env);
        storage
            .save_epoch_state(&genesis())
            .expect("epoch law stored");
        storage
            .save_max_report_age_ns(60_000_000_000)
            .expect("epoch mode activated");
        storage
            .create_pending_deposit(&lawful, 77)
            .expect("create deposit");
        let key = DepositPageKey { page: 0 };
        let mut bytes = env
            .storage()
            .persistent()
            .get::<_, soroban_sdk::Bytes>(&key)
            .expect("page stored")
            .to_alloc_vec();
        let offset = 5 + 4 + 4 + 8 + 32;
        bytes[offset] ^= 0x01;
        env.storage().persistent().set(
            &key,
            &soroban_sdk::Bytes::from_slice(&env, &bytes),
        );
        assert!(storage.pending_deposit_stats().is_err());
        assert!(storage.pending_deposit_oldest_epoch().is_err());
        assert!(storage.verify_pending_deposit_integrity().is_err());
        assert!(storage.create_pending_deposit(&lawful, 77).is_err());
    });

    // A tampered aggregate fails closed without touching pages.
    let env = Env::default();
    env.mock_all_auths();
    let id = contract_id(&env);
    env.as_contract(&id, || {
        let storage = SorobanStorage::new(&env);
        storage
            .save_epoch_state(&genesis())
            .expect("epoch law stored");
        storage
            .save_max_report_age_ns(60_000_000_000)
            .expect("epoch mode activated");
        storage
            .create_pending_deposit(&lawful, 77)
            .expect("create deposit");
        let inflated = PendingDepositAggregate {
            count: 1,
            total_assets: 2_000,
            oldest_epoch: Some(EpochId::new(1)),
        };
        env.storage().persistent().set(
            &AggregateKey,
            &soroban_sdk::Bytes::from_slice(
                &env,
                &encode_pending_deposit_aggregate(&inflated),
            ),
        );
        assert!(storage.pending_deposit_stats().is_err());
        assert!(storage.pending_deposit_oldest_epoch().is_err());
        assert!(storage.verify_pending_deposit_integrity().is_err());

        env.storage().persistent().remove(&AggregateKey);
        assert!(storage.pending_deposit_stats().is_err());
    });

    // A page stored under the wrong key fails closed on every read path.
    let env = Env::default();
    env.mock_all_auths();
    let id = contract_id(&env);
    env.as_contract(&id, || {
        let storage = SorobanStorage::new(&env);
        storage
            .save_epoch_state(&genesis())
            .expect("epoch law stored");
        storage
            .save_max_report_age_ns(60_000_000_000)
            .expect("epoch mode activated");
        storage
            .create_pending_deposit(&lawful, 77)
            .expect("create deposit");
        let real = env
            .storage()
            .persistent()
            .get::<_, soroban_sdk::Bytes>(&DepositPageKey { page: 0 })
            .expect("page stored");
        env.storage()
            .persistent()
            .set(&DepositPageKey { page: 1 }, &real);
        assert!(storage.verify_pending_deposit_integrity().is_err());
        assert!(storage.pending_deposit_stats().is_err());
    });
}

#[cfg(feature = "immediate-entrypoints")]
#[test]
fn legacy_withdrawal_quotes_are_dropped_and_fifo_identity_preserved() {
    let owner_a = test_address(7);
    let owner_b = test_address(8);
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&2u32.to_le_bytes());
    for (id, owner, receiver, shares, legacy_quote, seconds) in [
        (1u64, owner_a, owner_a, 500u128, 424_242_424_242u128, 1_600_000_000u64),
        (2u64, owner_b, owner_b, 700u128, 999_999_999u128, 1_600_000_001u64),
    ] {
        bytes.extend_from_slice(&id.to_le_bytes());
        bytes.extend_from_slice(owner.as_bytes());
        bytes.extend_from_slice(receiver.as_bytes());
        bytes.extend_from_slice(&shares.to_le_bytes());
        bytes.extend_from_slice(&legacy_quote.to_le_bytes());
        bytes.extend_from_slice(&ts(seconds).as_u64().to_le_bytes());
    }

    let decoded = super::decode_withdraw_queue_page(&bytes).expect("legacy page decodes");
    assert_eq!(decoded.len(), 2);

    // FIFO identity survives: position, ownership, custody, and time.
    assert_eq!(decoded[0].0, 1);
    assert_eq!(decoded[0].1.owner, owner_a);
    assert_eq!(decoded[0].1.receiver, owner_a);
    assert_eq!(decoded[0].1.escrow_shares, 500);
    assert_eq!(decoded[0].1.requested_at_ns, ts(1_600_000_000));
    assert_eq!(decoded[1].0, 2);
    assert_eq!(decoded[1].1.owner, owner_b);
    assert_eq!(decoded[1].1.receiver, owner_b);
    assert_eq!(decoded[1].1.escrow_shares, 700);
    assert_eq!(decoded[1].1.requested_at_ns, ts(1_600_000_001));

    // The legacy fixed asset claim never survives migration, and every
    // migrated entry is bound to the migration intake epoch.
    assert_eq!(decoded[0].1.min_assets_out, 0);
    assert_eq!(decoded[1].1.min_assets_out, 0);
    assert_eq!(decoded[0].1.epoch_id, templar_vault_kernel::EpochId::MIGRATION_INTAKE);
    assert_eq!(decoded[1].1.epoch_id, templar_vault_kernel::EpochId::MIGRATION_INTAKE);
    assert!(decoded[0].1.is_migrated_legacy());
    assert!(decoded[1].1.is_migrated_legacy());

    // Re-decoding is idempotent and the encoded live page carries no quote
    // authority anywhere in the migrated slots.
    let again = super::decode_withdraw_queue_page(&bytes).expect("legacy page decodes");
    assert_eq!(again, decoded);
    let reencoded = encode_withdraw_queue_page(decoded.iter().map(|(id, w)| (*id, w)));
    let decoded_again = super::decode_withdraw_queue_page(&reencoded).expect("redecode");
    assert_eq!(decoded_again, decoded);
}

#[test]
fn migrated_quote_authority_and_law_violations_reject_pages() {
    let owner_a = test_address(7);
    let withdrawal = PendingWithdrawal::new(
        owner_a,
        owner_a,
        500,
        123,
        ts(1_700_000_000),
        EpochId::new(1),
    )
    .expect("lawful withdrawal");
    let good = encode_withdraw_queue_page(core::iter::once((1u64, &withdrawal)));
    assert_eq!(
        super::decode_withdraw_queue_page(&good)
            .expect("epoch page decodes")
            .len(),
        1
    );

    // Zero escrow and the migration intake epoch with any quote are both
    // claim-authority violations and cannot enter a live page.
    assert!(
        PendingWithdrawal::new(
            owner_a,
            owner_a,
            0,
            0,
            ts(1_700_000_000),
            EpochId::new(1),
        )
        .is_err()
    );
    let mut zero_escrow = good.clone();
    zero_escrow[80..96].fill(0);
    assert!(super::decode_withdraw_queue_page(&zero_escrow).is_err());
    let mut forged_epoch = good.clone();
    forged_epoch[120] = 0;
    assert!(super::decode_withdraw_queue_page(&forged_epoch).is_err());

    // Truncation and extra bytes fail closed.
    let mut short = good.clone();
    short.pop();
    assert!(super::decode_withdraw_queue_page(&short).is_err());
    let mut long = good.clone();
    long.push(0);
    assert!(super::decode_withdraw_queue_page(&long).is_err());
}

#[test]
fn epoch_page_rejects_migrated_record_with_quote_authority() {
    let forged = PendingWithdrawal::migrated_legacy(
        test_address(7),
        test_address(7),
        500,
        ts(1_600_000_000),
    )
    .expect("migrated intake carries no quote");
    assert_eq!(forged.min_assets_out, 0);
    assert_eq!(forged.epoch_id, EpochId::MIGRATION_INTAKE);
    assert!(forged.is_migrated_legacy());

    // Forge a quote onto the migrated slot: a migration-intake record with
    // any positive floor is forged pre-settlement claim authority.
    let mut bytes = encode_withdraw_queue_page(core::iter::once((1u64, &forged)));
    let floor_off = 5 + 4 + 8 + 32 + 32 + 16;
    bytes[floor_off] = 0x01;
    assert!(super::decode_withdraw_queue_page(&bytes).is_err());

    // And a valid migrated page survives repeated encode/decode cycles
    // without ever reviving a claim: quotes stay zero across every round.
    let clean = encode_withdraw_queue_page(core::iter::once((1u64, &forged)));
    for _ in 0..3 {
        let decoded = super::decode_withdraw_queue_page(&clean).expect("redecode");
        assert_eq!(decoded.len(), 1);
        assert_eq!(decoded[0].1.min_assets_out, 0);
        assert_eq!(decoded[0].1.epoch_id, EpochId::MIGRATION_INTAKE);
    }
}

#[test]
fn deposit_record_law_rejects_zero_assets_and_migration_epoch() {
    assert!(
        PendingDeposit::new(test_address(7), 0, ts(1_700_000_000), EpochId::new(1)).is_err()
    );
    assert!(
        PendingDeposit::new(
            test_address(7),
            1_000,
            ts(1_700_000_000),
            EpochId::MIGRATION_INTAKE,
        )
        .is_err()
    );
    assert!(
        PendingDeposit::new(test_address(7), 1_000, ts(1_700_000_000), EpochId::new(1)).is_ok()
    );
}

#[test]
fn epoch_state_roundtrip_persists_and_rejects_tampering() {
    let env = Env::default();
    env.mock_all_auths();
    let id = contract_id(&env);
    env.as_contract(&id, || {
        let storage = SorobanStorage::new(&env);
        let open = storage.load_epoch_state().expect("absent epoch state");
        assert_eq!(open, genesis());
        storage.save_epoch_state(&open).expect("store epoch law");
        assert_eq!(storage.load_epoch_state().expect("load epoch law"), open);

        // Accepted reports can only enter while the vault is at cutoff for
        // the exact intake epoch. Drive the real settlement sequence.
        let cutoff = epoch_state(EpochPhase::Cutoff, 1, Some(1_700_000_000));
        let cutoff_state = storage
            .load_epoch_state()
            .expect("epoch law")
            .begin_cutoff(ts(1_700_000_000))
            .expect("cutoff law");
        storage.save_epoch_state(&cutoff_state).expect("store cutoff law");
        assert_eq!(cutoff_state, cutoff);
        assert_eq!(storage.load_epoch_state().expect("load cutoff law"), cutoff_state);

        // A settlement snapshot can only be stored through the kernel
        // settlement law against the stored state.
        storage
            .save_max_report_age_ns(3_600_000_000_000)
            .expect("freshness law configured");
        let accepted = report(1, 1_700_000_000);
        storage
            .record_accepted_report(EpochId::new(1), &accepted)
            .expect("accepted report stored");
        assert!(storage.any_accepted_reports().expect("accepted report presence"));
        assert_eq!(
            storage
                .load_accepted_report()
                .expect("accepted report read")
                .expect("accepted report present"),
            AcceptedReportRecord {
                settlement_epoch: EpochId::new(1),
                report: accepted,
            }
        );

        let snapshot = storage
            .load_epoch_state()
            .expect("epoch law")
            .build_settlement_snapshot(
                &accepted,
                1_000_000_000,
                1_000,
                ts(1_700_000_060),
                3_600_000_000_000,
            )
            .expect("lawful settlement snapshot");
        let settled = storage
            .load_epoch_state()
            .expect("epoch law")
            .apply_settled(&snapshot)
            .expect("kernel settlement law");
        assert_eq!(settled.phase, EpochPhase::Open);
        assert_eq!(
            settled
                .last_settled
                .as_ref()
                .expect("settled snapshot stored")
                .report_seq(),
            1
        );
        storage
            .save_epoch_state(&settled)
            .expect("settlement record stored");
        assert_eq!(storage.load_epoch_state().expect("load settled law"), settled);

        // Replaying the accepted report for the settled epoch fails: the
        // settlement advance removed the intake authority.
        assert!(
            storage
                .record_accepted_report(EpochId::new(1), &accepted)
                .is_err()
        );

        // Stored bytes must decode back exactly; corruption fails closed.
        let raw = storage
            .load_versioned(StorageKind::EpochState)
            .expect("read epoch record")
            .expect("epoch record stored");
        let mut tampered = raw.clone();
        let seq_off = 5 + 1 + 8 + 1 + 1 + 8;
        for byte in tampered[seq_off..seq_off + 8].iter_mut() {
            *byte = 0;
        }
        storage
            .save_versioned(StorageKind::EpochState, &tampered)
            .expect("tamper stored record");
        assert!(storage.load_epoch_state().is_err());
    });
}

#[test]
fn accepted_reports_are_governed_and_absent_is_lawful() {
    let env = Env::default();
    env.mock_all_auths();
    let id = contract_id(&env);
    env.as_contract(&id, || {
        let storage = SorobanStorage::new(&env);
        assert!(!storage.any_accepted_reports().expect("lawful absence"));
        assert!(storage.load_accepted_report().expect("lawful absence").is_none());
    });
}

#[test]
fn accepted_report_law_enforces_cutoff_and_sequence() {
    let env = Env::default();
    env.mock_all_auths();
    let id = contract_id(&env);
    env.as_contract(&id, || {
        let storage = SorobanStorage::new(&env);
        // Epoch mode is off until governance configures freshness law.
        assert!(!storage.epoch_mode_active());
        assert!(
            storage
                .record_accepted_report(EpochId::new(1), &report(1, 1_700_000_000))
                .is_err()
        );

        storage
            .save_max_report_age_ns(3_600_000_000_000)
            .expect("freshness law configured");
        assert!(storage.epoch_mode_active());

        // Intake is open: no report can bind an epoch that has not reached
        // its cutoff.
        storage
            .save_epoch_state(&genesis())
            .expect("store epoch law");
        assert!(
            storage
                .record_accepted_report(EpochId::new(1), &report(1, 1_700_000_000))
                .is_err()
        );

        let cutoff_seconds = 1_700_000_000u64;
        let cutoff_state = storage
            .load_epoch_state()
            .expect("epoch law")
            .begin_cutoff(ts(cutoff_seconds))
            .expect("cutoff law");
        storage
            .save_epoch_state(&cutoff_state)
            .expect("store cutoff law");

        // Reports bound before the cutoff or for the wrong epoch are
        // refused.
        assert!(
            storage
                .record_accepted_report(
                    EpochId::new(1),
                    &report(1, cutoff_seconds - 1),
                )
                .is_err()
        );
        assert!(
            storage
                .record_accepted_report(EpochId::new(9), &report(1, cutoff_seconds))
                .is_err()
        );

        // The first accepted report is stored exactly and is visible.
        storage
            .record_accepted_report(EpochId::new(1), &report(2, cutoff_seconds))
            .expect("accepted report stored");
        assert!(storage.any_accepted_reports().expect("accepted report presence"));
        let stored = storage
            .load_accepted_report()
            .expect("accepted report read")
            .expect("accepted report present");
        assert_eq!(stored.settlement_epoch, EpochId::new(1));
        assert_eq!(stored.report.report_seq, 2);

        // A replayed or stale sequence is refused forever: the same epoch
        // cannot be re-accepted.
        assert!(
            storage
                .record_accepted_report(EpochId::new(1), &report(3, cutoff_seconds))
                .is_err()
        );
    });
}

#[test]
fn accepted_report_record_codec_rejects_law_violations() {
    let good = encode_accepted_report_record(&AcceptedReportRecord {
        settlement_epoch: EpochId::new(1),
        report: report(7, 1_700_000_000),
    })
    .expect("encode accepted record");
    assert_eq!(
        decode_accepted_report_record(&good)
            .expect("decode accepted record")
            .report
            .report_seq,
        7
    );

    // An accepted report cannot bind a non-settlement epoch and cannot
    // carry a zero sequence.
    assert!(
        encode_accepted_report_record(&AcceptedReportRecord {
            settlement_epoch: EpochId::MIGRATION_INTAKE,
            report: report(1, 1_700_000_000),
        })
        .is_err()
    );
    assert!(
        encode_accepted_report_record(&AcceptedReportRecord {
            settlement_epoch: EpochId::new(1),
            report: report(0, 1_700_000_000),
        })
        .is_err()
    );

    let mut tampered = good.clone();
    tampered[5] ^= 0x01;
    assert!(decode_accepted_report_record(&tampered).is_err());
    let mut epoch_zero = good.clone();
    epoch_zero[5] = 0;
    assert!(decode_accepted_report_record(&epoch_zero).is_err());
    let mut short = good.clone();
    short.pop();
    assert!(decode_accepted_report_record(&short).is_err());
    let mut long = good.clone();
    long.push(0);
    assert!(decode_accepted_report_record(&long).is_err());
}

#[test]
fn epoch_record_codec_rejects_law_violations() {
    let good = encode_epoch_record(&genesis()).expect("encode epoch law");
    assert_eq!(
        decode_epoch_record(&good).expect("decode epoch law"),
        genesis()
    );

    // A settlement snapshot cannot be stored against the wrong intake
    // epoch or with a zero supply: both violate the settlement law.
    let bad_settlement = settlement_state(1_700_000_000, Some(1_700_000_000));
    let encoded_bad = encode_epoch_record(&bad_settlement).expect("encode settlement");
    let mut tampered = encoded_bad.clone();
    let supply_off = 5 + 1 + 8 + 1 + 1 + 8 + 8 + 32 + 8 + 16;
    for byte in tampered[supply_off..supply_off + 16].iter_mut() {
        *byte = 0;
    }
    assert!(decode_epoch_record(&tampered).is_err());

    // An Open epoch with a cutoff binding violates the cutoff law.
    let open_with_cutoff = epoch_state(EpochPhase::Open, 1, Some(1_700_000_000));
    assert!(encode_epoch_record(&open_with_cutoff).is_err());
    // A Cutoff epoch without a cutoff binding cannot be stored either.
    let cutoff_without_binding = epoch_state(EpochPhase::Cutoff, 1, None);
    assert!(encode_epoch_record(&cutoff_without_binding).is_err());

    let mut short = good.clone();
    short.pop();
    assert!(decode_epoch_record(&short).is_err());
    let mut long = good.clone();
    long.push(0);
    assert!(decode_epoch_record(&long).is_err());
    let mut magic = good.clone();
    magic[0] ^= 0xff;
    assert!(decode_epoch_record(&magic).is_err());
}

#[test]
fn max_report_age_gate_requires_governance_configuration() {
    let env = Env::default();
    env.mock_all_auths();
    let id = contract_id(&env);
    env.as_contract(&id, || {
        let storage = SorobanStorage::new(&env);
        assert!(!storage.epoch_mode_active());
        assert_eq!(
            storage.load_max_report_age_ns().expect("lawful absence"),
            None
        );
        assert_eq!(
            storage.load_epoch_state().expect("lawful absence"),
            genesis()
        );

        // A zero bound would silently disable staleness law: rejected.
        assert!(storage.save_max_report_age_ns(0).is_err());
        storage
            .save_max_report_age_ns(3_600_000_000_000)
            .expect("freshness law configured");
        assert_eq!(
            storage
                .load_max_report_age_ns()
                .expect("freshness law read"),
            Some(3_600_000_000_000)
        );
        assert!(storage.epoch_mode_active());
        storage
            .save_max_report_age_ns(3_600_000_000_000)
            .expect("identical configuration is idempotent");
        assert!(storage.save_max_report_age_ns(7_200_000_000_000).is_err());
        assert_eq!(
            storage
                .load_max_report_age_ns()
                .expect("immutable freshness law read"),
            Some(3_600_000_000_000)
        );

        // A tampered record fails closed: the mode cannot silently stay
        // active on corrupt governance configuration.
        let raw = storage
            .load_versioned(StorageKind::MaxReportAge)
            .expect("read config")
            .expect("config stored");
        let mut tampered = raw.clone();
        tampered[5..13].fill(0);
        storage
            .save_versioned(StorageKind::MaxReportAge, &tampered)
            .expect("tamper stored config");
        assert!(!storage.epoch_mode_active());
        assert!(storage.load_max_report_age_ns().is_err());

    });
}

#[test]
fn u64_records_are_typed_and_validated() {
    let kind = StorageKind::MaxReportAge;
    let bytes = encode_u64_record(kind, 42).expect("encode record");
    assert_eq!(decode_u64_record(&bytes, kind).expect("decode record"), 42);
    assert!(encode_u64_record(kind, 0).is_err());
    let mut tampered = bytes.clone();
    tampered[5..13].fill(0);
    assert!(decode_u64_record(&tampered, kind).is_err());
    let mut wrong = bytes.clone();
    wrong[3] = StorageKind::EpochState.tag();
    assert!(decode_u64_record(&wrong, kind).is_err());
    let mut long = bytes.clone();
    long.push(0);
    assert!(decode_u64_record(&long, kind).is_err());
    let mut short = bytes.clone();
    short.pop();
    assert!(decode_u64_record(&short, kind).is_err());
}

#[test]
fn vault_state_header_mirrors_epoch_law_and_divergence_fails_closed() {
    let env = Env::default();
    env.mock_all_auths();
    let id = contract_id(&env);
    env.as_contract(&id, || {
        let mut storage = SorobanStorage::new(&env);
        let state = templar_vault_kernel::VaultState {
            epoch: storage.load_epoch_state().expect("epoch law"),
            ..templar_vault_kernel::VaultState::default()
        };
        use super::Storage;
        storage.save_state(&state).expect("vault state stored");
        assert_eq!(
            storage.load_state().expect("vault state read"),
            Some(state.clone())
        );

        // The header mirror and the dedicated epoch record agree exactly.
        let stored = env
            .storage()
            .persistent()
            .get::<_, soroban_sdk::Bytes>(&super::SorobanStorageKey::StateBlob)
            .expect("state stored")
            .to_alloc_vec();
        assert_eq!(
            epoch_state_from_state_header_blob(&stored).expect("header epoch"),
            state.epoch
        );

        // A cutoff epoch cannot be smuggled into a stored header: the
        // binding is required and re-validated on every read.
        let cutoff_state = templar_vault_kernel::VaultState {
            epoch: state
                .epoch
                .begin_cutoff(ts(1_700_000_000))
                .expect("cutoff law"),
            ..state.clone()
        };
        storage
            .save_state(&cutoff_state)
            .expect("cutoff state stored");
        assert_eq!(
            storage.load_state().expect("vault state read"),
            Some(cutoff_state.clone())
        );
        assert_eq!(
            storage.load_epoch_state().expect("epoch law"),
            cutoff_state.epoch
        );

        // If the dedicated epoch record diverges from the header mirror,
        // epoch reads fail closed rather than trust either side.
        let epoch_key = StorageKind::EpochState;
        let raw = storage
            .load_versioned(epoch_key)
            .expect("read epoch record")
            .expect("epoch record stored");
        let mut diverged = raw.clone();
        let intake_off = 5 + 1 + 8;
        diverged[intake_off] = 3;
        diverged[intake_off + 1] = 0;
        diverged[intake_off + 2] = 0;
        diverged[intake_off + 3] = 0;
        diverged[intake_off + 4] = 0;
        diverged[intake_off + 5] = 0;
        diverged[intake_off + 6] = 0;
        diverged[intake_off + 7] = 0;
        storage
            .save_versioned(epoch_key, &diverged)
            .expect("tamper epoch record");
        assert!(storage.load_epoch_state().is_err());
    });
}

#[must_use]
fn market_entry(target_id: u32) -> (u32, MarketConfig) {
    (target_id, MarketConfig::new(true, 1_000, None))
}

#[must_use]
fn adapter_bindings(
    env: &Env,
    pairs: &[(u32, soroban_sdk::Address)],
) -> soroban_sdk::Map<u32, soroban_sdk::Address> {
    let mut bindings = Map::new(env);
    for (target_id, adapter) in pairs {
        bindings.set(*target_id, adapter.clone());
    }
    bindings
}

fn store_adapter_bindings(env: &Env, bindings: &soroban_sdk::Map<u32, soroban_sdk::Address>) {
    env.storage()
        .instance()
        .set(&crate::contract::VaultDataKey::AdapterBindings, bindings);
}

#[rstest::rstest]
#[case([3, 1, 2])]
#[case([u32::MAX, 0, 1 << 31])]
fn adapter_binding_enumeration_is_complete_and_ascending(#[case] ids: [u32; 3]) {
    let env = Env::default();
    env.mock_all_auths();
    let id = contract_id(&env);
    env.as_contract(&id, || {
        let storage = SorobanStorage::new(&env);
        storage
            .save_policy_markets(&encode_markets(&OrderedMap::from_iter([
                market_entry(ids[0]),
                market_entry(ids[1]),
                market_entry(ids[2]),
            ])))
            .expect("markets stored");
        let adapter_one = soroban_sdk::Address::generate(&env);
        let adapter_two = soroban_sdk::Address::generate(&env);
        let adapter_three = soroban_sdk::Address::generate(&env);
        store_adapter_bindings(
            &env,
            &adapter_bindings(
                &env,
                &[
                    (ids[0], adapter_three.clone()),
                    (ids[1], adapter_one.clone()),
                    (ids[2], adapter_two.clone()),
                ],
            ),
        );

        // Every persisted binding is enumerated exactly once, and the order
        // is ascending by TargetId regardless of insertion order.
        let enumerated = storage
            .enumerate_market_adapter_bindings()
            .expect("lawful enumeration");
        assert_eq!(enumerated.len(), 3);
        assert!(
            enumerated.windows(2).all(|pair| pair[0].0 < pair[1].0),
            "enumeration must be strictly ascending by TargetId"
        );
        assert_eq!(
            enumerated,
            alloc::vec::Vec::from([
                (ids[1], adapter_one),
                (ids[2], adapter_two),
                (ids[0], adapter_three),
            ])
        );
    });
}

#[test]
fn adapter_binding_enumeration_fails_closed_without_records() {
    let env = Env::default();
    env.mock_all_auths();
    let id = contract_id(&env);
    env.as_contract(&id, || {
        let storage = SorobanStorage::new(&env);
        // Nothing persisted at all: never a silently empty enumeration.
        assert!(storage.enumerate_market_adapter_bindings().is_err());

        // Exposed markets with no binding record: still fails closed.
        storage
            .save_policy_markets(&encode_markets(&OrderedMap::from_iter([market_entry(1)])))
            .expect("markets stored");
        assert!(storage.enumerate_market_adapter_bindings().is_err());

        // A bound target that was never exposed cannot settle: the
        // enumeration rejects it instead of dropping it silently.
        store_adapter_bindings(
            &env,
            &adapter_bindings(
                &env,
                &[(9, soroban_sdk::Address::generate(&env))],
            ),
        );
        assert!(storage.enumerate_market_adapter_bindings().is_err());
    });
}

#[test]
fn adapter_binding_enumeration_rejects_orphan_and_corrupt_records() {
    let env = Env::default();
    env.mock_all_auths();
    let id = contract_id(&env);
    env.as_contract(&id, || {
        let storage = SorobanStorage::new(&env);
        storage
            .save_policy_markets(&encode_markets(&OrderedMap::from_iter([
                market_entry(1),
                market_entry(2),
            ])))
            .expect("markets stored");
        let adapter_one = soroban_sdk::Address::generate(&env);
        let adapter_nine = soroban_sdk::Address::generate(&env);

        // A binding to a target that was never exposed is corruption.
        store_adapter_bindings(
            &env,
            &adapter_bindings(
                &env,
                &[(1, adapter_one), (9, adapter_nine)],
            ),
        );
        assert!(storage.enumerate_market_adapter_bindings().is_err());

        // A torn markets record fails closed; it must never degrade into an
        // empty or partial enumeration.
        let mut torn = encode_markets(&OrderedMap::from_iter([market_entry(1)]));
        torn.pop();
        storage
            .save_policy_markets(&torn)
            .expect("torn markets stored");
        assert!(storage.enumerate_market_adapter_bindings().is_err());
    });
}

#[test]
fn markets_codec_rejects_duplicate_targets_and_corruption() {
    let good = encode_markets(&OrderedMap::from_iter([
        market_entry(1),
        market_entry(2),
    ]));
    assert!(decode_markets(&good).is_ok());

    // Re-pointing the second record onto the first target would silently
    // collapse two markets into one: the decoder must reject the duplicate.
    let mut duplicate = good.clone();
    duplicate[31..35].copy_from_slice(&1u32.to_le_bytes());
    assert!(decode_markets(&duplicate).is_err());

    let mut magic = good.clone();
    magic[0] ^= 0xff;
    assert!(decode_markets(&magic).is_err());

    let mut kind = good.clone();
    kind[3] ^= 0x01;
    assert!(decode_markets(&kind).is_err());

    let mut short = good.clone();
    short.pop();
    assert!(decode_markets(&short).is_err());

    let mut long = good.clone();
    long.push(0);
    assert!(decode_markets(&long).is_err());
}

#[cfg(all(test, not(feature = "immediate-entrypoints")))]
#[test]
fn epoch_only_profile_rejects_legacy_immediate_state_and_pages() {
    // A fresh-deploy epoch target never migrates immediate vault state:
    // pre-epoch queue pages and v2 state headers must fail closed, never
    // be silently decoded into epoch law or genesis-epoch authority.

    // Pre-epoch withdrawal queue page (exact immediate-vault encoding).
    let owner_a = test_address(7);
    let mut legacy_page = Vec::new();
    legacy_page.extend_from_slice(&1u32.to_le_bytes());
    legacy_page.extend_from_slice(&9u64.to_le_bytes());
    legacy_page.extend_from_slice(owner_a.as_bytes());
    legacy_page.extend_from_slice(owner_a.as_bytes());
    legacy_page.extend_from_slice(&500u128.to_le_bytes());
    legacy_page.extend_from_slice(&424_242_242_242u128.to_le_bytes());
    legacy_page.extend_from_slice(&ts(1_600_000_000).as_u64().to_le_bytes());
    assert!(super::decode_withdraw_queue_page(&legacy_page).is_err());

    // Genuine pre-epoch v2 vault-state header envelope: rejected by the
    // version gate, never read back as a genesis-epoch state header.
    let mut v2_blob = Vec::new();
    v2_blob.extend_from_slice(b"TVS");
    v2_blob.push(1u8);
    v2_blob.push(2u8);
    v2_blob.extend_from_slice(&[7u8; 160]);
    assert!(epoch_state_from_state_header_blob(&v2_blob).is_err());

    // The on-ledger law path also fails closed: immediate state bytes
    // persisted under the state-blob key never compose into vault state.
    use super::Storage as _;
    let env = Env::default();
    let id = contract_id(&env);
    env.as_contract(&id, || {
        env.storage().persistent().set(
            &super::SorobanStorageKey::StateBlob,
            &soroban_sdk::Bytes::from_slice(&env, &v2_blob),
        );
        let storage = SorobanStorage::new(&env);
        assert!(storage.load_state().is_err());
    });
}
