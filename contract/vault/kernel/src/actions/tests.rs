use super::{
    convert_to_assets, convert_to_assets_ceil, convert_to_shares, convert_to_shares_ceil,
};
use crate::state::vault::{VaultConfig, VaultState, MAX_PENDING};
use crate::FeesSpec;

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
