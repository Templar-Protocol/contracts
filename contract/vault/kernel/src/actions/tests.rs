use super::{
    convert_to_assets, convert_to_assets_ceil, convert_to_shares, convert_to_shares_ceil,
};
use crate::state::vault::{FeeAccrualAnchor, VaultConfig, VaultState, MAX_PENDING};
use crate::{apply_action, compute_fee_shares_from_assets, FeesSpec, KernelAction, Number};
use crate::effects::{KernelEffect, KernelEvent};
use crate::fee::FeeSlot;
use crate::math::wad::{compute_management_fee_shares, Wad, YEAR_NS};
use crate::error::KernelError;

fn base_config() -> VaultConfig {
    VaultConfig {
        fees: FeesSpec::zero(),
        min_withdrawal_assets: 0,
        withdrawal_cooldown_ns: 0,
        max_pending_withdrawals: MAX_PENDING as u32,
        paused: false,
        virtual_shares: 0,
        virtual_assets: 0,
    }
}

fn base_state(total_assets: u128, total_shares: u128) -> VaultState {
    let mut state = VaultState::new();
    state.total_assets = total_assets;
    state.total_shares = total_shares;
    state.idle_assets = total_assets;
    state
}

#[test]
fn convert_to_shares_ceil_matches_floor_on_exact_multiple() {
    let config = base_config();
    let state = base_state(100, 100);
    let assets = 40;
    let floor = convert_to_shares(&state, &config, assets);
    let ceil = convert_to_shares_ceil(&state, &config, assets);
    assert_eq!(floor, ceil);
}

#[test]
fn convert_to_shares_ceil_rounds_up_on_fractional() {
    let config = base_config();
    let state = base_state(3, 2);
    let assets = 1;
    let floor = convert_to_shares(&state, &config, assets);
    let ceil = convert_to_shares_ceil(&state, &config, assets);
    assert_eq!(ceil, floor.saturating_add(1));
}

#[test]
fn convert_to_shares_ceil_is_floor_or_floor_plus_one() {
    let config = base_config();
    let cases = [(1, 3, 2), (5, 7, 11), (10, 25, 9), (12, 19, 23)];
    for (assets, total_assets, total_shares) in cases {
        let state = base_state(total_assets, total_shares);
        let floor = convert_to_shares(&state, &config, assets);
        let ceil = convert_to_shares_ceil(&state, &config, assets);
        assert!(ceil >= floor);
        assert!(ceil <= floor.saturating_add(1));
    }
}

#[test]
fn convert_to_assets_ceil_matches_floor_on_exact_multiple() {
    let config = base_config();
    let state = base_state(100, 100);
    let shares = 25;
    let floor = convert_to_assets(&state, &config, shares);
    let ceil = convert_to_assets_ceil(&state, &config, shares);
    assert_eq!(floor, ceil);
}

#[test]
fn convert_to_assets_ceil_rounds_up_on_fractional() {
    let config = base_config();
    let state = base_state(2, 3);
    let shares = 1;
    let floor = convert_to_assets(&state, &config, shares);
    let ceil = convert_to_assets_ceil(&state, &config, shares);
    assert_eq!(ceil, floor.saturating_add(1));
}

#[test]
fn convert_to_assets_ceil_is_floor_or_floor_plus_one() {
    let config = base_config();
    let cases = [(1, 2, 3), (7, 13, 9), (5, 11, 19), (9, 17, 23)];
    for (shares, total_assets, total_shares) in cases {
        let state = base_state(total_assets, total_shares);
        let floor = convert_to_assets(&state, &config, shares);
        let ceil = convert_to_assets_ceil(&state, &config, shares);
        assert!(ceil >= floor);
        assert!(ceil <= floor.saturating_add(1));
    }
}

#[test]
fn deposit_overflow_total_assets_rejected() {
    let config = base_config();
    let mut state = base_state(u128::MAX - 5, 1);
    state.idle_assets = state.total_assets;
    let result = apply_action(
        state,
        &config,
        None,
        &[0u8; 32],
        KernelAction::Deposit {
            owner: [1u8; 32],
            receiver: [2u8; 32],
            assets_in: 10,
            min_shares_out: 0,
            now_ns: 0,
        },
    );
    assert!(matches!(
        result,
        Err(KernelError::InvalidState("deposit would overflow total_assets"))
    ));
}

#[test]
fn deposit_overflow_total_shares_rejected() {
    let config = base_config();
    let mut state = base_state(u128::MAX - 1, u128::MAX);
    state.idle_assets = state.total_assets;
    let result = apply_action(
        state,
        &config,
        None,
        &[0u8; 32],
        KernelAction::Deposit {
            owner: [1u8; 32],
            receiver: [2u8; 32],
            assets_in: 1,
            min_shares_out: 0,
            now_ns: 0,
        },
    );
    assert!(matches!(
        result,
        Err(KernelError::InvalidState("minting would overflow total_shares"))
    ));
}

#[test]
fn refresh_fees_overflow_total_supply_rejected() {
    let mut config = base_config();
    config.fees = FeesSpec::new(
        FeeSlot::new(Wad::one() / 2, [9u8; 32]),
        FeeSlot::new(Wad::zero(), [8u8; 32]),
        None,
    );
    let mut state = base_state(1_000, u128::MAX - 1);
    state.fee_anchor = FeeAccrualAnchor::new(0, 0);

    let result = apply_action(
        state,
        &config,
        None,
        &[0u8; 32],
        KernelAction::RefreshFees { now_ns: 1 },
    );
    assert!(matches!(
        result,
        Err(KernelError::InvalidState(
            "fee minting would overflow total_supply"
        ))
    ));
}

#[test]
fn execute_withdraw_skips_zero_expected_assets() {
    let config = base_config();
    let mut state = base_state(1_000, 1_000);
    let owner = [3u8; 32];
    let receiver = [4u8; 32];
    let escrow_shares = 500;

    state
        .withdraw_queue
        .enqueue(
            owner,
            receiver,
            escrow_shares,
            0,
            0,
            config.max_pending_withdrawals,
        )
        .expect("enqueue");

    let self_id = [9u8; 32];
    let result = apply_action(
        state,
        &config,
        None,
        &self_id,
        KernelAction::ExecuteWithdraw { now_ns: 0 },
    )
    .expect("execute_withdraw");

    assert!(result.state.op_state.is_idle());
    assert!(result.state.withdraw_queue.is_empty());

    assert!(result.effects.iter().any(|effect| {
        matches!(
            effect,
            KernelEffect::TransferShares { from, to, shares }
                if *from == self_id && *to == owner && *shares == escrow_shares
        )
    }));
    assert!(result.effects.iter().any(|effect| {
        matches!(
            effect,
            KernelEffect::EmitEvent {
                event: KernelEvent::WithdrawalSkipped {
                    id: _,
                    owner: who,
                    receiver: dest,
                    escrow_shares: shares,
                    expected_assets: 0,
                },
            } if *who == owner && *dest == receiver && *shares == escrow_shares
        )
    }));
}

fn minted_shares_for(effects: &[KernelEffect], owner: [u8; 32]) -> u128 {
    effects
        .iter()
        .filter_map(|effect| match effect {
            KernelEffect::MintShares { owner: who, shares } if *who == owner => Some(*shares),
            _ => None,
        })
        .sum()
}

#[test]
fn refresh_fees_respects_growth_rate_cap_with_both_fee_types() {
    let management_recipient = [9u8; 32];
    let performance_recipient = [8u8; 32];

    let mut config = base_config();
    config.fees = FeesSpec::new(
        FeeSlot::new(Wad::one() / 5, performance_recipient),
        FeeSlot::new(Wad::one() / 10, management_recipient),
        Some(Wad::one() / 10),
    );

    let mut state = base_state(2_000, 1_000);
    state.fee_anchor = FeeAccrualAnchor::new(1_000, 0);

    let result = apply_action(
        state,
        &config,
        None,
        &[0u8; 32],
        KernelAction::RefreshFees { now_ns: YEAR_NS },
    )
    .unwrap();

    let capped_total_assets = 1_100;
    let mgmt_shares = compute_management_fee_shares(
        capped_total_assets,
        2_000,
        1_000,
        config.fees.management.fee_wad,
        0,
        YEAR_NS,
    );
    let mgmt_expected: u128 = mgmt_shares.into();
    let total_supply_after_mgmt = 1_000u128.saturating_add(mgmt_expected);

    let profit = capped_total_assets.saturating_sub(1_000);
    let fee_assets = config
        .fees
        .performance
        .fee_wad
        .apply_floored(Number::from(profit));
    let perf_shares = compute_fee_shares_from_assets(
        fee_assets,
        Number::from(2_000u128),
        Number::from(total_supply_after_mgmt),
    );
    let perf_expected: u128 = perf_shares.into();

    let mgmt_minted = minted_shares_for(&result.effects, management_recipient);
    let perf_minted = minted_shares_for(&result.effects, performance_recipient);
    assert_eq!(mgmt_minted, mgmt_expected);
    assert_eq!(perf_minted, perf_expected);

    let uncapped_mgmt_shares = compute_management_fee_shares(
        2_000,
        2_000,
        1_000,
        config.fees.management.fee_wad,
        0,
        YEAR_NS,
    );
    let uncapped_mgmt: u128 = uncapped_mgmt_shares.into();
    assert!(mgmt_minted < uncapped_mgmt);
}

#[test]
fn refresh_fees_rejects_non_advancing_timestamp() {
    let mut config = base_config();
    config.fees = FeesSpec::zero();
    let mut state = base_state(1_000, 1_000);
    state.fee_anchor = FeeAccrualAnchor::new(1_000, 500);

    let result = apply_action(
        state,
        &config,
        None,
        &[0u8; 32],
        KernelAction::RefreshFees { now_ns: 500 },
    );

    assert!(matches!(
        result,
        Err(KernelError::InvalidState("fee refresh timestamp must advance"))
    ));
}
