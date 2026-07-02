//! Fuzz the vault kernel's fee-accrual and share-conversion math — the
//! fund-critical arithmetic the `dev` merge reworked
//! (`contract/vault/kernel/src/math/wad.rs`,
//! `contract/vault/kernel/src/actions/mod.rs`). All oracles are *failable
//! properties* of the real functions (P1), not a re-implementation (P2):
//!
//! * `total_assets_for_fee_accrual` never reports a basis above current assets,
//!   passes current assets through unchanged when uncapped, and stays at or
//!   above the anchor once the rate cap engages on real growth.
//! * The management- and performance-fee share calculations return zero in
//!   their documented degenerate cases (zero fee / zero supply / no elapsed
//!   time / no profit) and never panic.
//! * Each `convert_to_*_bounded` is a faithful capped view of its unbounded
//!   sibling: whenever it returns `Ok(v)`, `v <= cap` and `v` equals the
//!   unbounded conversion (within the cap the quotient fits `u128`, so the
//!   bounded and unbounded results coincide exactly).
//!
//! MUTATION-CHECK (P5): in `total_assets_for_fee_accrual` (wad.rs), change the
//! final `cur_total_assets.min(max_total_assets)` to `max_total_assets` — then
//! an input with `max_total_assets > cur_total_assets` makes the basis exceed
//! current assets and the `capped <= cur` assertion below must fire.

#![no_main]

use arbitrary::Arbitrary;
use libfuzzer_sys::fuzz_target;
use templar_vault_kernel::error::InvalidStateCode;
use templar_vault_kernel::{
    compute_fee_shares, compute_management_fee_shares, convert_to_assets,
    convert_to_assets_bounded, convert_to_assets_ceil, convert_to_assets_ceil_bounded,
    convert_to_shares, convert_to_shares_bounded, convert_to_shares_ceil,
    convert_to_shares_ceil_bounded, total_assets_for_fee_accrual, FeesSpec, Number, VaultConfig,
    VaultState, Wad,
};

#[derive(Arbitrary, Debug)]
struct FeeMathInput {
    cur_total_assets: u128,
    anchor_total_assets: u128,
    anchor_ts: u64,
    now_ns: u64,
    last_ts: u64,
    // `None` exercises the uncapped pass-through path.
    max_rate: Option<u128>,
    fee_assets_base: u128,
    management_fee_wad: u128,
    performance_fee_wad: u128,
    last_total_assets: u128,
    total_supply: u128,
    virtual_shares: u128,
    virtual_assets: u128,
    convert_amount: u128,
    convert_cap: u128,
}

fuzz_target!(|input: FeeMathInput| {
    // --- fee-accrual basis -------------------------------------------------
    let cur = input.cur_total_assets;
    let anchor = input.anchor_total_assets;
    let max_rate = input.max_rate.map(Wad::from);
    let capped = total_assets_for_fee_accrual(cur, anchor, input.anchor_ts, input.now_ns, max_rate);

    assert!(
        capped <= cur,
        "fee-accrual basis ({capped}) exceeds current assets ({cur})",
    );
    if max_rate.is_none() {
        assert_eq!(
            capped, cur,
            "uncapped accrual must pass current assets through"
        );
    }
    if max_rate.is_some() && cur > anchor && input.now_ns >= input.anchor_ts {
        assert!(
            capped >= anchor,
            "capped basis ({capped}) fell below the anchor ({anchor})",
        );
    }

    // --- management fee shares --------------------------------------------
    let mgmt = compute_management_fee_shares(
        input.fee_assets_base,
        cur,
        input.total_supply,
        Wad::from(input.management_fee_wad),
        input.last_ts,
        input.now_ns,
    );
    if input.management_fee_wad == 0 || input.total_supply == 0 || input.now_ns <= input.last_ts {
        assert!(
            mgmt.is_zero(),
            "management fee must be zero with no fee / no supply / no elapsed time",
        );
    }

    // --- performance fee shares -------------------------------------------
    let perf = compute_fee_shares(
        Number::from(cur),
        Number::from(input.last_total_assets),
        Wad::from(input.performance_fee_wad),
        Number::from(input.total_supply),
    );
    if cur <= input.last_total_assets || input.performance_fee_wad == 0 || input.total_supply == 0 {
        assert!(
            perf.is_zero(),
            "performance fee must be zero with no profit / no fee / no supply",
        );
    }

    // --- bounded vs unbounded conversions (differential) ------------------
    let state = VaultState {
        total_assets: cur,
        total_shares: input.total_supply,
        ..VaultState::default()
    };
    let config = VaultConfig {
        fees: FeesSpec::default(),
        min_withdrawal_assets: 0,
        withdrawal_cooldown_ns: 0,
        max_pending_withdrawals: 0,
        paused: false,
        virtual_shares: input.virtual_shares,
        virtual_assets: input.virtual_assets,
    };
    let amount = input.convert_amount;
    let cap = input.convert_cap;
    let err = InvalidStateCode::Unknown;
    let max_cap = u128::MAX;

    if let (Ok(floor), Ok(ceil)) = (
        convert_to_shares_bounded(&state, &config, amount, max_cap, err),
        convert_to_shares_ceil_bounded(&state, &config, amount, max_cap, err),
    ) {
        assert!(
            floor <= ceil,
            "convert_to_shares floor ({floor}) exceeded ceil ({ceil})",
        );
        assert!(
            ceil <= floor.saturating_add(1),
            "convert_to_shares ceil ({ceil}) was more than floor + 1 ({floor})",
        );
    }

    if let (Ok(floor), Ok(ceil)) = (
        convert_to_assets_bounded(&state, &config, amount, max_cap, err),
        convert_to_assets_ceil_bounded(&state, &config, amount, max_cap, err),
    ) {
        assert!(
            floor <= ceil,
            "convert_to_assets floor ({floor}) exceeded ceil ({ceil})",
        );
        assert!(
            ceil <= floor.saturating_add(1),
            "convert_to_assets ceil ({ceil}) was more than floor + 1 ({floor})",
        );
    }

    if let Ok(v) = convert_to_shares_bounded(&state, &config, amount, cap, err) {
        assert!(v <= cap, "shares: bounded result exceeded cap");
        assert_eq!(
            v,
            convert_to_shares(&state, &config, amount),
            "shares: bounded result diverged from unbounded within cap",
        );
    }
    if let Ok(v) = convert_to_assets_bounded(&state, &config, amount, cap, err) {
        assert!(v <= cap, "assets: bounded result exceeded cap");
        assert_eq!(
            v,
            convert_to_assets(&state, &config, amount),
            "assets: bounded result diverged from unbounded within cap",
        );
    }
    if let Ok(v) = convert_to_shares_ceil_bounded(&state, &config, amount, cap, err) {
        assert!(v <= cap, "shares(ceil): bounded result exceeded cap");
        assert_eq!(
            v,
            convert_to_shares_ceil(&state, &config, amount),
            "shares(ceil): bounded result diverged from unbounded within cap",
        );
    }
    if let Ok(v) = convert_to_assets_ceil_bounded(&state, &config, amount, cap, err) {
        assert!(v <= cap, "assets(ceil): bounded result exceeded cap");
        assert_eq!(
            v,
            convert_to_assets_ceil(&state, &config, amount),
            "assets(ceil): bounded result diverged from unbounded within cap",
        );
    }
});
