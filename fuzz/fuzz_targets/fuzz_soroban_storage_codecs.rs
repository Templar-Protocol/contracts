#![no_main]

use arbitrary::Arbitrary;
use libfuzzer_sys::fuzz_target;
use templar_curator_primitives::policy::market_lock::{
    FencingToken, LeaseOwner, MarketLease, MarketLeaseRegistry,
};
use templar_curator_primitives::policy::state::{MarketConfig, OrderedMap};
use templar_curator_primitives::policy::supply_queue::{SupplyQueue, SupplyQueueEntry};
use templar_soroban_runtime::test_utils::fuzz_api;
use templar_vault_kernel::{
    Address, AllocationPlanEntry, FeeAccrualAnchor, OpState, Restrictions, TargetId, TimestampNs,
    VaultState, VersionedState, WithdrawQueue, WithdrawingState,
};

#[derive(Arbitrary, Debug)]
struct StorageCodecInput {
    raw: Vec<u8>,
    addresses: Vec<[u8; 32]>,
    restriction_mode: u8,
    max_length: Option<u16>,
    queue_entries: Vec<(u32, u128, u8)>,
    market_entries: Vec<(u32, bool, u128, Option<Vec<u8>>)>,
    principal_entries: Vec<(u32, u128)>,
    lease_entries: Vec<(u32, u64, Option<u64>, u64, u64, u64)>,
    withdraw_entries: Vec<(u64, [u8; 32], [u8; 32], u128, u128, u64)>,
    op_tag: u8,
    next_op_id: u64,
    total_assets: u128,
    total_shares: u128,
    idle_assets: u128,
    external_assets: u128,
    fee_anchor_assets: u128,
    fee_anchor_ts: u64,
}

fn build_restrictions(input: &StorageCodecInput) -> Restrictions {
    let addresses = input
        .addresses
        .iter()
        .copied()
        .map(Address)
        .collect::<Vec<_>>();
    match input.restriction_mode % 2 {
        0 => Restrictions::blacklist(addresses),
        _ => Restrictions::whitelist(addresses),
    }
}

fn build_supply_queue(input: &StorageCodecInput) -> SupplyQueue {
    let entries = input
        .queue_entries
        .iter()
        .filter_map(|(target, amount, priority)| {
            SupplyQueueEntry::new_with_priority(*target, *amount, *priority).ok()
        })
        .collect::<Vec<_>>();
    let max_length = input
        .max_length
        .and_then(|value| u32::from(value).try_into().ok());
    SupplyQueue::try_from_entries(entries, max_length).unwrap_or_default()
}

fn build_markets(input: &StorageCodecInput) -> OrderedMap<TargetId, MarketConfig> {
    let mut out = OrderedMap::new();
    for (target_id, enabled, cap, _cap_group_bytes) in &input.market_entries {
        let _ = out.insert(*target_id, MarketConfig::new(*enabled, *cap, None));
    }
    out
}

fn build_principals(input: &StorageCodecInput) -> OrderedMap<TargetId, u128> {
    let mut out = OrderedMap::new();
    for (target_id, principal) in &input.principal_entries {
        let _ = out.insert(*target_id, *principal);
    }
    out
}

fn build_leases(input: &StorageCodecInput) -> MarketLeaseRegistry {
    let mut leases = OrderedMap::new();
    for (target_id, owner, op_id, acquired_at, expires_at, fencing_token) in &input.lease_entries {
        let lease = MarketLease::from_parts(
            *target_id,
            LeaseOwner(*owner),
            *op_id,
            TimestampNs(*acquired_at),
            TimestampNs(*expires_at),
            FencingToken(*fencing_token),
        );
        let _ = leases.insert(*target_id, lease);
    }
    MarketLeaseRegistry::from_parts(leases, 1)
}

fn build_withdraw_queue(input: &StorageCodecInput) -> WithdrawQueue {
    let entries = input
        .withdraw_entries
        .iter()
        .map(
            |(id, owner, receiver, escrow_shares, expected_assets, requested_at_ns)| {
                (
                    *id,
                    templar_vault_kernel::PendingWithdrawal::new(
                        Address(*owner),
                        Address(*receiver),
                        *escrow_shares,
                        *expected_assets,
                        TimestampNs(*requested_at_ns),
                    ),
                )
            },
        )
        .collect::<Vec<_>>();
    WithdrawQueue::with_state(entries, 0, input.withdraw_entries.len() as u64)
}

fn build_op_state(input: &StorageCodecInput) -> OpState {
    match input.op_tag % 3 {
        0 => OpState::Idle,
        1 => OpState::Withdrawing(WithdrawingState {
            op_id: 1,
            request_id: 2,
            index: 0,
            remaining: input.total_assets,
            collected: input.idle_assets,
            receiver: Address([4u8; 32]),
            owner: Address([5u8; 32]),
            escrow_shares: input.total_shares,
        }),
        _ => OpState::Allocating(templar_vault_kernel::AllocatingState {
            op_id: 1,
            index: 0,
            remaining: input.external_assets,
            plan: vec![AllocationPlanEntry::new(0, input.external_assets)],
        }),
    }
}

fn build_versioned_state(input: &StorageCodecInput) -> VersionedState {
    VersionedState::new(VaultState {
        total_assets: input.total_assets,
        total_shares: input.total_shares,
        idle_assets: input.idle_assets,
        external_assets: input.external_assets,
        fee_anchor: FeeAccrualAnchor::new(
            input.fee_anchor_assets,
            TimestampNs(input.fee_anchor_ts),
        ),
        op_state: build_op_state(input),
        withdraw_queue: build_withdraw_queue(input),
        next_op_id: input.next_op_id,
    })
}

fuzz_target!(|input: StorageCodecInput| {
    let _ = fuzz_api::decode_restrictions_bytes(&input.raw);
    let _ = fuzz_api::decode_supply_queue_bytes(&input.raw);
    let _ = fuzz_api::decode_markets_bytes(&input.raw);
    let _ = fuzz_api::decode_principals_bytes(&input.raw);
    let _ = fuzz_api::decode_policy_locks_bytes(&input.raw);
    let _ = fuzz_api::decode_state_blob_bytes(&input.raw);

    let restrictions = build_restrictions(&input);
    let restrictions_bytes = fuzz_api::encode_restrictions_bytes(&restrictions);
    let decoded_restrictions =
        fuzz_api::decode_restrictions_bytes(&restrictions_bytes).expect("restrictions roundtrip");
    assert_eq!(decoded_restrictions, restrictions);

    let supply_queue = build_supply_queue(&input);
    let supply_queue_bytes = fuzz_api::encode_supply_queue_bytes(&supply_queue);
    let decoded_supply_queue =
        fuzz_api::decode_supply_queue_bytes(&supply_queue_bytes).expect("supply queue roundtrip");
    assert_eq!(decoded_supply_queue, supply_queue);

    let markets = build_markets(&input);
    let markets_bytes = fuzz_api::encode_markets_bytes(&markets);
    let decoded_markets =
        fuzz_api::decode_markets_bytes(&markets_bytes).expect("markets roundtrip");
    assert_eq!(decoded_markets, markets);

    let principals = build_principals(&input);
    let principals_bytes = fuzz_api::encode_principals_bytes(&principals);
    let decoded_principals =
        fuzz_api::decode_principals_bytes(&principals_bytes).expect("principals roundtrip");
    assert_eq!(decoded_principals, principals);

    let leases = build_leases(&input);
    let lease_bytes = fuzz_api::encode_policy_locks_bytes(&leases);
    let decoded_leases =
        fuzz_api::decode_policy_locks_bytes(&lease_bytes).expect("leases roundtrip");
    assert_eq!(decoded_leases, leases);

    let state = build_versioned_state(&input);
    let state_bytes = fuzz_api::encode_state_blob_bytes(&state);
    let decoded_state = fuzz_api::decode_state_blob_bytes(&state_bytes).expect("state roundtrip");
    assert_eq!(decoded_state, state);
});
