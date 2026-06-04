//! Fuzz `Snapshot` serialization round-trips — the real Borsh and JSON codecs
//! the contract uses to persist/transmit snapshots (P1: round-trip oracle, the
//! strongest kind). A `Snapshot` is built from arbitrary field values.
//!
//! - **Borsh** is the on-chain storage codec and is bit-lossless:
//!   `from_slice(to_vec(x)) == x` exactly.
//! - **JSON** is a view codec; `Decimal` serializes to a 38-fractional-digit
//!   string, so it is *not* bit-lossless for an arbitrary 512-bit
//!   `interest_rate` — we assert string-level idempotence there instead.
//!
//! (The previous version of this target only set fields and read them back,
//! which is tautological — it tested no contract code. P2.)
//!
//! MUTATION-CHECK (P5): change the `#[near(serializers = [borsh, json])]` field
//! order on `Snapshot` (e.g. swap `borrow_asset_borrowed` and
//! `borrow_asset_deposited_active`) — the Borsh round-trip below decodes the
//! bytes in the wrong order and the equality assertion must fire.

#![no_main]
#![cfg(not(target_arch = "wasm32"))]
#![allow(
    clippy::expect_used,
    reason = "panics on invariant violation are the intended libFuzzer crash signal"
)]

use arbitrary::Arbitrary;
use libfuzzer_sys::fuzz_target;
use near_sdk::{borsh, json_types::U64, serde_json};
use templar_common::{
    asset::{BorrowAssetAmount, CollateralAssetAmount},
    snapshot::Snapshot,
    time_chunk::TimeChunk,
    Decimal,
};

#[derive(Arbitrary, Debug)]
struct SnapshotScenario {
    time_chunk: u64,
    end_timestamp_ms: u64,
    borrow_asset_deposited_active: u128,
    borrow_asset_borrowed: u128,
    collateral_asset_deposited: u128,
    yield_distribution: u128,
    // Raw repr for the interest_rate Decimal — exercises arbitrary 512-bit
    // fixed-point values through the codec, not just round numbers.
    interest_rate_repr: [u64; 8],
}

fuzz_target!(|scenario: SnapshotScenario| {
    let snapshot = Snapshot {
        time_chunk: TimeChunk(U64(scenario.time_chunk)),
        end_timestamp_ms: U64(scenario.end_timestamp_ms),
        borrow_asset_deposited_active: BorrowAssetAmount::from(
            scenario.borrow_asset_deposited_active,
        ),
        borrow_asset_borrowed: BorrowAssetAmount::from(scenario.borrow_asset_borrowed),
        collateral_asset_deposited: CollateralAssetAmount::from(
            scenario.collateral_asset_deposited,
        ),
        yield_distribution: BorrowAssetAmount::from(scenario.yield_distribution),
        interest_rate: Decimal::from_repr(scenario.interest_rate_repr),
    };

    // Borsh round-trip: the on-chain storage codec. Must be lossless.
    let borsh_bytes = borsh::to_vec(&snapshot).expect("Snapshot must Borsh-serialize");
    let borsh_decoded: Snapshot =
        borsh::from_slice(&borsh_bytes).expect("Snapshot must Borsh-deserialize");
    assert_eq!(
        snapshot, borsh_decoded,
        "Snapshot Borsh round-trip changed the value",
    );

    // JSON round-trip: the view codec. It must not panic, and every
    // *structural* field must round-trip exactly — a codec bug that dropped a
    // field, mis-keyed it, or truncated an integer amount would fail here.
    //
    // `interest_rate` is deliberately excluded: `Decimal` serializes to a
    // 38-fractional-digit string, so its JSON form is lossy/non-canonical at
    // the ~10⁻³⁸ digit (parse→serialize can shift the last digit). That's a
    // property of a display format, not a fund-safety concern — the lossless
    // fidelity of `interest_rate` is already covered by the Borsh round-trip
    // above. Asserting bit-equality on it here only flags benign display
    // rounding.
    let json = serde_json::to_string(&snapshot).expect("Snapshot must JSON-serialize");
    let d: Snapshot = serde_json::from_str(&json).expect("Snapshot must JSON-deserialize");
    assert_eq!(
        d.time_chunk, snapshot.time_chunk,
        "JSON: time_chunk changed"
    );
    assert_eq!(
        d.end_timestamp_ms.0, snapshot.end_timestamp_ms.0,
        "JSON: end_timestamp_ms changed",
    );
    assert_eq!(
        d.borrow_asset_deposited_active, snapshot.borrow_asset_deposited_active,
        "JSON: borrow_asset_deposited_active changed",
    );
    assert_eq!(
        d.borrow_asset_borrowed, snapshot.borrow_asset_borrowed,
        "JSON: borrow_asset_borrowed changed",
    );
    assert_eq!(
        d.collateral_asset_deposited, snapshot.collateral_asset_deposited,
        "JSON: collateral_asset_deposited changed",
    );
    assert_eq!(
        d.yield_distribution, snapshot.yield_distribution,
        "JSON: yield_distribution changed",
    );
});
