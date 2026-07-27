#![allow(unexpected_cfgs)]
#![cfg(artifact_wasm)]

use soroban_sdk::{
    testutils::Address as _, Address, Bytes, Env, Executable, IntoVal, String, Symbol,
};
use templar_soroban_shared_types::{
    RuntimeVersionResponse, RUNTIME_DEFAULT_FEATURE_FLAGS, RUNTIME_V1_FEATURE_FLAGS,
    RUNTIME_V1_VERSION,
};

const CURATOR_PROXY_WASM: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../../../target/wasm32-unknown-unknown/release-soroban/templar_curator_proxy_soroban.wasm"
));
const RUNTIME_WASM: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../../../target/wasm32-unknown-unknown/release-soroban/templar_soroban_runtime.wasm"
));

fn wasm_hash(address: &Address) -> soroban_sdk::BytesN<32> {
    match address.executable() {
        Some(Executable::Wasm(hash)) => hash,
        executable => panic!("expected Wasm executable, got {executable:?}"),
    }
}

#[test]
fn optimized_proxy_tracks_the_runtime_artifact_transition() {
    let env = Env::default();
    env.cost_estimate().budget().reset_unlimited();
    env.mock_all_auths();
    let initialization_authority = Address::generate(&env);
    let legacy_target = env.register(CURATOR_PROXY_WASM, (&initialization_authority,));
    let proxy = env.register(CURATOR_PROXY_WASM, (&initialization_authority,));
    let governance = Address::generate(&env);
    let legacy_hash = wasm_hash(&legacy_target);

    env.invoke_contract::<()>(
        &proxy,
        &Symbol::new(&env, "initialize_legacy_v1"),
        (&legacy_target, &governance, &legacy_hash).into_val(&env),
    );

    assert_eq!(
        env.invoke_contract::<RuntimeVersionResponse>(
            &proxy,
            &Symbol::new(&env, "vault_version"),
            soroban_sdk::vec![&env],
        ),
        (
            String::from_str(&env, RUNTIME_V1_VERSION),
            RUNTIME_V1_FEATURE_FLAGS,
        )
    );

    let runtime_hash = env
        .deployer()
        .upload_contract_wasm(Bytes::from_slice(&env, RUNTIME_WASM));
    assert_ne!(runtime_hash, legacy_hash);
    env.as_contract(&legacy_target, || {
        env.deployer()
            .update_current_contract_wasm(runtime_hash.clone());
    });
    assert_eq!(wasm_hash(&legacy_target), runtime_hash);

    assert_eq!(
        env.invoke_contract::<RuntimeVersionResponse>(
            &proxy,
            &Symbol::new(&env, "vault_version"),
            soroban_sdk::vec![&env],
        ),
        (
            String::from_str(&env, templar_soroban_runtime::RUNTIME_VERSION),
            RUNTIME_DEFAULT_FEATURE_FLAGS,
        )
    );
}
