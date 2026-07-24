#![allow(unexpected_cfgs)]
#![cfg(artifact_wasm)]

use soroban_sdk::{contract, testutils::Address as _, Address, Env, IntoVal, Symbol};

const CUSTODIAL_ADAPTER_WASM: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../../../target/wasm32-unknown-unknown/release-soroban/templar_soroban_custodial_adapter.wasm"
));

#[contract]
struct DummyVault;

#[test]
fn optimized_custodial_adapter_exports_reported_at() {
    let env = Env::default();
    env.cost_estimate().budget().reset_unlimited();
    let admin = Address::generate(&env);
    let vault = env.register(DummyVault, ());
    let custodian = Address::generate(&env);
    let asset_admin = Address::generate(&env);
    let asset = env
        .register_stellar_asset_contract_v2(asset_admin)
        .address();
    let adapter = env.register(CUSTODIAL_ADAPTER_WASM, (&admin, &vault, &custodian, &asset));

    assert_eq!(
        env.invoke_contract::<Option<u64>>(
            &adapter,
            &Symbol::new(&env, "reported_at"),
            (&asset,).into_val(&env),
        ),
        None
    );
}
