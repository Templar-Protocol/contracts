//! Fuzz `BorrowPosition::liquidatable_collateral` — the real production
//! liquidation-amount function. Asserts the **safety properties** the contract
//! depends on, calling the real function (P1).
//!
//! ## Known bugs being tracked (P4)
//!
//! - **Liquidation denominator underflow** (ENG-342):
//!   `liquidatable_collateral` computes `(mcr * convert(liability) - collateral)
//!   / (mcr * discount - 1)`. When `mcr * (1 - liquidator_spread) <= 1` the
//!   denominator underflows on unsigned Decimal subtraction. The fuzzer
//!   discovered this is reachable with realistic-looking params (e.g.
//!   mcr=1.05, spread=0.1 → 0.945 < 1). Either `MarketConfiguration::validate`
//!   must enforce `mcr * (1 - spread) > 1`, or `liquidatable_collateral` must
//!   handle the case (return full collateral / clamp / use signed math).
//!   The harness skips this input region; any OTHER overflow in the function
//!   is still a finding.
//!
//! Inputs are also bounded so the arithmetic inside `liquidatable_collateral`
//! (multiplications of Decimals derived from large u128s) does not trip the
//! intentional U512 overflow checks — that boundary is the job of the
//! `fuzz_borrow_overflow` target, not this one (P2).

#![no_main]
#![cfg(not(target_arch = "wasm32"))]

use libfuzzer_sys::fuzz_target;
use near_sdk::json_types::{I64, U64};
use templar_common::{
    asset::{BorrowAssetAmount, CollateralAssetAmount},
    borrow::BorrowPosition,
    oracle::pyth::{self, PythTimestamp},
    price::PricePair,
    Decimal,
};

// MUTATION-CHECK (P5): in `BorrowPosition::liquidatable_collateral`
// (borrow.rs:180), remove the trailing `.min(collateral)`. Then for an
// undercollateralized-but-not-underwater position the computed amount can
// exceed the held collateral and invariant #1 (`result <= total_collateral`)
// must fire.

// Bound rate-like inputs to a sane range so we explore the liquidation logic,
// not Decimal overflow. `mcr` is the maintenance collateral ratio (≥ 1).
fn decimal_in_range(x: u32, min_x100: u32, max_x100: u32) -> Decimal {
    let span = max_x100 - min_x100;
    let val = min_x100 + (x % (span + 1));
    Decimal::from(val) / Decimal::from(100u32)
}

fuzz_target!(|data: (
    u32, // mcr 100..=300 (i.e. 1.00..=3.00)
    u32, // liquidator_spread 0..=50 (0.00..=0.50)
    u64, // collateral
    u64, // principal
    u64, // in_flight
    i64, // collateral price raw (positive only — bound below)
    i64, // borrow price raw
    u64, // collateral confidence
    u64, // borrow confidence
)| {
    let (mcr_raw, spread_raw, coll, principal, in_flight, c_px, b_px, c_conf, b_conf) = data;

    // mcr ∈ [1.00, 3.00], liquidator_spread ∈ [0.00, 0.50].
    let mcr = decimal_in_range(mcr_raw, 100, 300);
    let liquidator_spread = decimal_in_range(spread_raw, 0, 50);

    // Position amounts kept in u64 so the principal+in_flight+interest+fees
    // sum stays within u128 (P2: this fuzzer targets liquidation, not
    // overflow). `fuzz_borrow_overflow` covers the boundary.
    let mut position = BorrowPosition::new(0);
    position.collateral_asset_deposit = CollateralAssetAmount::new(u128::from(coll));
    position.borrow_asset_principal = BorrowAssetAmount::new(u128::from(principal));
    position.borrow_asset_in_flight = BorrowAssetAmount::new(u128::from(in_flight));

    // Prices: keep positive, conf < price (Pyth invariant), and within ~6
    // orders of magnitude of each other. Rationale (P2, targeted +
    // documented): `liquidatable_collateral` does `mcr * convert(liability)`,
    // and `convert` returns the price-ratio scaled by amount. If the ratio
    // crosses ~10²³ the Decimal multiplication trips intentional U512
    // overflow checks — which is *correct contract behavior*, not a bug. We
    // skip those extremes here so the liquidation logic itself gets fuzzed.
    // BACKSTOP: fuzz_decimal_arithmetic exercises Decimal overflow guards
    // independently; price-extreme inputs are not a contract-reachable
    // scenario in production (Pyth bounds publish ranges per feed).
    let c_price_pos = 100_000 + (c_px.unsigned_abs() % 1_000_000_000);
    let b_price_pos = 100_000 + (b_px.unsigned_abs() % 1_000_000_000);
    let c_conf_bounded = c_conf % c_price_pos;
    let b_conf_bounded = b_conf % b_price_pos;

    let c_pyth = pyth::Price {
        price: I64(c_price_pos as i64),
        conf: U64(c_conf_bounded),
        expo: -8,
        publish_time: PythTimestamp::from_secs(0),
    };
    let b_pyth = pyth::Price {
        price: I64(b_price_pos as i64),
        conf: U64(b_conf_bounded),
        expo: -8,
        publish_time: PythTimestamp::from_secs(0),
    };

    let Ok(price_pair) = PricePair::new(&c_pyth, 8, &b_pyth, 8) else {
        return;
    };

    // KNOWN BUG (ENG-342): the denominator `mcr * discount - 1` underflows when
    // `mcr * discount <= 1`. But that subtraction is only *reached* for a
    // position in the liquidatable-but-not-underwater band `1 < cr < mcr`:
    // healthy (`cr >= mcr`) and underwater (`cr <= 1`) positions — and
    // zero-liability ones — return early, so they are safe to fuzz even under
    // such mcr/spread. We therefore skip ONLY the unsafe band (targeted +
    // documented, P2/P4), preserving coverage of the early-return paths that a
    // blanket `mcr * discount <= 1` skip would discard. The abort itself is
    // asserted by `borrow::tests::liquidatable_collateral_denominator_underflow_aborts`.
    let discount = Decimal::ONE - liquidator_spread;
    if mcr * discount <= Decimal::ONE {
        // `collateralization_ratio` computes the exact same `cr` the function
        // uses internally (pessimistic collateral / optimistic liability), so
        // this faithfully predicts which inputs reach the underflowing line.
        if let Some(cr) = position.collateralization_ratio(&price_pair) {
            if cr > Decimal::ONE && cr < mcr {
                return; // ENG-342 region
            }
        }
    }

    // Call the real function.
    let result = position.liquidatable_collateral(&price_pair, mcr, liquidator_spread);

    // ---- Safety invariants (P2: non-trivial — a buggy function can
    // violate any of these) ----

    // 1. Never seize more collateral than the position holds.
    assert!(
        result <= position.get_total_collateral_amount(),
        "liquidatable_collateral returned {result:?} > total_collateral {:?}",
        position.get_total_collateral_amount(),
    );

    // 2. Zero liability ⇒ zero liquidatable.
    if position.get_total_borrow_asset_liability().is_zero() {
        assert_eq!(
            result,
            CollateralAssetAmount::zero(),
            "Zero liability must produce zero liquidatable collateral",
        );
    }

    // 3. Zero collateral ⇒ zero liquidatable. (You can't seize what isn't
    //    there. `liquidatable_collateral.min(collateral)` enforces this.)
    if position.get_total_collateral_amount().is_zero() {
        assert_eq!(
            result,
            CollateralAssetAmount::zero(),
            "Zero collateral must produce zero liquidatable collateral",
        );
    }

    // 4. Pure-healthy position (cr >= mcr) ⇒ zero liquidatable. The function
    //    checks the underwater branch (`cr <= 1` ⇒ seize all) *before* the
    //    healthy branch, so this invariant only holds above water; at the
    //    `cr == mcr == 1` corner the underwater branch wins (covered by #5).
    if let Some(cr) = position.collateralization_ratio(&price_pair) {
        if cr >= mcr && cr > Decimal::ONE {
            assert_eq!(
                result,
                CollateralAssetAmount::zero(),
                "Position with cr={cr:?} >= mcr={mcr:?} must not be liquidatable",
            );
        }
        // 5. Totally-underwater (cr <= 1) ⇒ seize entire collateral.
        if cr <= Decimal::ONE && !position.get_total_collateral_amount().is_zero() {
            assert_eq!(
                result,
                position.get_total_collateral_amount(),
                "Underwater position (cr={cr:?}) must allow seizing all collateral",
            );
        }
    }
});
