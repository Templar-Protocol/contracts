#![no_main]
#![allow(
    clippy::expect_used,
    reason = "panics on invariant violation are the intended libFuzzer crash signal"
)]

// MUTATION-CHECK (P5): in `contract/vault/soroban/src/storage/mod.rs`, change
// one encoder to drop a field (e.g. omit `push_u128(amount)` in
// `encode_supply_queue`) or change a `push_u32` width. Then the corresponding
// `decode(encode(x)) == x` round-trip assertion below must fire.

use arbitrary::Arbitrary;
use libfuzzer_sys::fuzz_target;
use templar_curator_primitives::policy::cap_group::{CapGroup, CapGroupId, CapGroupRecord};
use templar_curator_primitives::policy::market_lock::{
    FencingToken, LeaseOwner, MarketLease, MarketLeaseRegistry,
};
use templar_curator_primitives::policy::state::{MarketConfig, OrderedMap};
use templar_curator_primitives::policy::supply_queue::{SupplyQueue, SupplyQueueEntry};
use templar_soroban_runtime::test_utils::fuzz_api;
use templar_vault_kernel::{
    Address, AllocationPlanEntry, FeeAccrualAnchor, OpState, Restrictions, TargetId, TimestampNs,
    VaultState, WithdrawQueue, WithdrawingState,
};

// Cap collection sizes so encoded blobs stay below libFuzzer's RSS ceiling.
// The decoders themselves don't enforce upper bounds; that should be tracked
// separately (untrusted-input DoS).
const MAX_COLLECTION_LEN: usize = 64;

// (owner, receiver, escrow_shares, expected_assets, requested_at_ns).
// The pending-withdrawal id is *not* an input dimension: the decoder requires
// strictly-ascending, contiguous-from-head ids (see `build_withdraw_queue`), so
// an arbitrary id would only ever abort construction or fail the round-trip.
type WithdrawEntry = ([u8; 32], [u8; 32], u128, u128, u64);

// (id_bytes, principal, absolute_cap, relative_cap). `relative_cap` is a `u128`
// rather than a raw `Wad`: the encoder persists it via `as_u128_trunc`, so a
// `Wad` above 2^128 would not survive the round-trip (a display-precision
// artifact of the wire format, not a codec bug). See `build_cap_groups`.
type CapGroupEntry = (Vec<u8>, u128, Option<u128>, Option<u128>);

#[derive(Arbitrary, Debug)]
struct StorageCodecInput {
    addresses: Vec<[u8; 32]>,
    restriction_mode: u8,
    max_length: Option<u16>,
    queue_entries: Vec<(u32, u128, u8)>,
    market_entries: Vec<(u32, bool, u128, Option<Vec<u8>>)>,
    principal_entries: Vec<(u32, u128)>,
    cap_group_entries: Vec<CapGroupEntry>,
    lease_entries: Vec<(u32, u64, Option<u64>, u64, u64, u64)>,
    withdraw_entries: Vec<WithdrawEntry>,
    // Starting id for the withdraw queue, so ids span non-zero values and cross
    // `WITHDRAW_QUEUE_PAGE_SIZE` page boundaries (exercises the V2 paged codec).
    withdraw_base_id: u64,
    op_tag: u8,
    refresh_plan: Vec<u32>,
    next_op_id: u64,
    total_assets: u128,
    total_shares: u128,
    idle_assets: u128,
    external_assets: u128,
    fee_anchor_assets: u128,
    fee_anchor_ts: u64,
}

fn truncate<T>(v: &[T], max: usize) -> &[T] {
    &v[..v.len().min(max)]
}

fn build_restrictions(input: &StorageCodecInput) -> Restrictions {
    let addresses = truncate(&input.addresses, MAX_COLLECTION_LEN)
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
    let entries = truncate(&input.queue_entries, MAX_COLLECTION_LEN)
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
    for (target_id, enabled, cap, cap_group_bytes) in
        truncate(&input.market_entries, MAX_COLLECTION_LEN)
    {
        let cap_group_id = cap_group_bytes
            .as_ref()
            .and_then(|raw| core::str::from_utf8(raw).ok())
            .map(str::to_owned)
            .and_then(|raw| CapGroupId::try_from(raw).ok());
        let _ = out.insert(*target_id, MarketConfig::new(*enabled, *cap, cap_group_id));
    }
    out
}

fn build_principals(input: &StorageCodecInput) -> OrderedMap<TargetId, u128> {
    let mut out = OrderedMap::new();
    for (target_id, principal) in truncate(&input.principal_entries, MAX_COLLECTION_LEN) {
        let _ = out.insert(*target_id, *principal);
    }
    out
}

// Map arbitrary bytes onto the CapGroupId alphabet (lowercase ascii, digits,
// `-`, `_`; 1..=64 chars) so most inputs yield a *valid* id and actually
// exercise the codec rather than being filtered out.
fn sanitize_cap_group_id(raw: &[u8]) -> Option<CapGroupId> {
    const ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789-_";
    if raw.is_empty() {
        return None;
    }
    let id: String = raw
        .iter()
        .take(64)
        .map(|byte| ALPHABET[*byte as usize % ALPHABET.len()] as char)
        .collect();
    CapGroupId::try_from(id).ok()
}

fn build_cap_groups(input: &StorageCodecInput) -> OrderedMap<CapGroupId, CapGroupRecord> {
    let mut out = OrderedMap::new();
    for (id_bytes, principal, absolute_cap, relative_cap) in
        truncate(&input.cap_group_entries, MAX_COLLECTION_LEN)
    {
        let Some(id) = sanitize_cap_group_id(id_bytes) else {
            continue;
        };
        let mut cap = CapGroup::default();
        cap.set_absolute_cap(*absolute_cap);
        // Construct the relative cap from a `u128` so the encoder's
        // `as_u128_trunc` is exact and the round-trip is lossless.
        cap.set_relative_cap(relative_cap.map(templar_vault_kernel::Wad::from));
        let _ = out.insert(
            id,
            CapGroupRecord {
                cap,
                principal: *principal,
            },
        );
    }
    out
}

fn build_leases(input: &StorageCodecInput) -> MarketLeaseRegistry {
    let mut leases = OrderedMap::new();
    for (target_id, owner, op_id, acquired_at, expires_at, fencing_token) in
        truncate(&input.lease_entries, MAX_COLLECTION_LEN)
    {
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
    let bounded = truncate(&input.withdraw_entries, MAX_COLLECTION_LEN);
    // `from_sorted_entries` aborts on unsorted/duplicate ids, and the storage
    // decoder additionally requires the head id to equal
    // `next_withdraw_to_execute` with every id in
    // `[next_withdraw_to_execute, next_pending_withdrawal_id)` and strictly
    // ascending. Assign contiguous ids `base..base+n` so the queue is always a
    // well-formed value the codec must round-trip losslessly — the payload
    // fields it actually serializes still come from the fuzz input. `base` is
    // free so ids straddle page boundaries in the V2 paged codec. (Rejection of
    // hostile/sparse ids is decode-side and out of scope; see the fuzz body.)
    let base = input
        .withdraw_base_id
        .min(u64::MAX - MAX_COLLECTION_LEN as u64);
    let entries = bounded
        .iter()
        .enumerate()
        .map(
            |(index, (owner, receiver, escrow_shares, expected_assets, requested_at_ns))| {
                (
                    base + index as u64,
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
    let next_pending = base + entries.len() as u64;
    WithdrawQueue::with_state(entries, base, next_pending)
}

fn build_op_state(input: &StorageCodecInput) -> OpState {
    match input.op_tag % 5 {
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
        2 => OpState::Allocating(templar_vault_kernel::AllocatingState {
            op_id: 1,
            index: 0,
            remaining: input.external_assets,
            plan: vec![AllocationPlanEntry::new(0, input.external_assets)],
        }),
        3 => OpState::Refreshing(templar_vault_kernel::RefreshingState {
            op_id: 1,
            index: 0,
            plan: truncate(&input.refresh_plan, MAX_COLLECTION_LEN).to_vec(),
        }),
        _ => OpState::Payout(templar_vault_kernel::PayoutState {
            op_id: 1,
            request_id: 2,
            receiver: Address([6u8; 32]),
            amount: input.total_assets,
            owner: Address([7u8; 32]),
            escrow_shares: input.total_shares,
            burn_shares: input.idle_assets,
        }),
    }
}

fn build_vault_state(input: &StorageCodecInput) -> VaultState {
    VaultState {
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
    }
}

fuzz_target!(|input: StorageCodecInput| {
    // Decoding arbitrary bytes is not fuzzed: the decoders over-allocate on an
    // unbounded length prefix (ENG-345). Only the encode→decode round-trips run.
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

    let cap_groups = build_cap_groups(&input);
    let cap_groups_bytes = fuzz_api::encode_cap_groups_bytes(&cap_groups);
    let decoded_cap_groups =
        fuzz_api::decode_cap_groups_bytes(&cap_groups_bytes).expect("cap groups roundtrip");
    // `CapGroupRecord` has no `PartialEq`; compare per entry. `CapGroup` is
    // `PartialEq`, and `relative_cap` round-trips exactly because it was built
    // from a `u128` (see `build_cap_groups`).
    assert_eq!(
        decoded_cap_groups.len(),
        cap_groups.len(),
        "cap groups: length changed",
    );
    for (id, record) in cap_groups.iter() {
        let decoded = decoded_cap_groups
            .get(id)
            .expect("cap groups: id missing after roundtrip");
        assert_eq!(decoded.cap, record.cap, "cap groups: cap changed");
        assert_eq!(
            decoded.principal, record.principal,
            "cap groups: principal changed",
        );
    }

    let leases = build_leases(&input);
    let lease_bytes = fuzz_api::encode_policy_locks_bytes(&leases);
    let decoded_leases =
        fuzz_api::decode_policy_locks_bytes(&lease_bytes).expect("leases roundtrip");
    assert_eq!(decoded_leases, leases);

    let state = build_vault_state(&input);

    // Legacy V1 monolithic blob (kept for regression).
    let state_bytes = fuzz_api::encode_state_blob_bytes(&state);
    let decoded_state = fuzz_api::decode_state_blob_bytes(&state_bytes).expect("state roundtrip");
    assert_eq!(decoded_state, state);

    // Production V2 paged format: header blob + per-page withdraw-queue codecs,
    // reassembled via `compose_state_from_header`.
    let decoded_paged =
        fuzz_api::roundtrip_state_paged_bytes(&state).expect("paged state roundtrip");
    assert_eq!(decoded_paged, state);
});
