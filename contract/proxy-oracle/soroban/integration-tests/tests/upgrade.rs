#![allow(
    clippy::should_panic_without_expect,
    clippy::doc_markdown,
    clippy::similar_names,
    clippy::large_digit_groups,
    clippy::explicit_iter_loop
)]

//! Upgrade boundaries.
//!
//! The adapter test mounts the catalog-pinned 0.1.4 Wasm and the current
//! compiled Wasm, swaps the executable through its owner-gated endpoint, and
//! exercises storage and historical reads afterward. `test-integration` builds
//! the current adapter fixture; the release loader verifies the old bytes.
//! Runtime/governance configurations are fresh-stack-only: these tests do not
//! claim an in-place runtime or governance upgrade.

use soroban_sdk::testutils::{Address as _, BytesN as _};
use soroban_sdk::{Address, BytesN};
use templar_proxy_oracle_soroban_governance_common::GovernanceAction;
use templar_proxy_oracle_soroban_integration_tests::common::Bootstrap;

// The ordinary fast gate stays native-only; Soroban test-integration and CI
// explicitly enable this proof after building the current optimized Wasm.
#[cfg(feature = "compiled-wasm-tests")]
mod compiled_adapter {
    use soroban_sdk::testutils::{Address as _, MockAuth, MockAuthInvoke};
    use soroban_sdk::xdr::ToXdr;
    use soroban_sdk::{Address, Env, Executable, IntoVal, Val, Vec};
    use templar_contract_artifacts::{fetch::released_soroban_bytes, SorobanArtifactId};
    use templar_proxy_oracle_soroban_common::{ContractError, PriceData};
    use templar_proxy_oracle_soroban_governance_common::GovernanceAction;
    use templar_proxy_oracle_soroban_integration_tests::common::{ledger, Bootstrap};
    use templar_proxy_oracle_soroban_sep40_adapter_contract::Sep40AdapterClient;

    fn compiled_adapter_wasms(
    ) -> Result<(std::vec::Vec<u8>, std::vec::Vec<u8>), Box<dyn std::error::Error>> {
        let artifact = SorobanArtifactId::Sep40Adapter;
        let old = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?
            .block_on(released_soroban_bytes(artifact, "0.1.4"))?;
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../../target/proxy-oracle-soroban/wasm")
            .join(format!(
                "{}.optimized.wasm",
                artifact.metadata().cargo_target_name
            ));
        let new = std::fs::read(path).map_err(|error| format!(
        "prepare the current fixture with `just -f contract/proxy-oracle/soroban/justfile optimize-sep40-adapter`: {error}"
    ))?;
        Ok((old, new))
    }

    fn authorize_adapter(
        env: &Env,
        owner: &Address,
        adapter: &Address,
        function: &str,
        args: Vec<Val>,
    ) {
        env.mock_auths(&[MockAuth {
            address: owner,
            invoke: &MockAuthInvoke {
                contract: adapter,
                fn_name: function,
                args,
                sub_invokes: &[],
            },
        }]);
    }

    #[test]
    #[allow(
        clippy::too_many_lines,
        reason = "keeps old/new executable and active/decommissioned state assertions in one swap scenario"
    )]
    fn compiled_adapter_upgrade_preserves_state_and_serves_newest_distinct_buckets() {
        let (old_wasm, new_wasm) = compiled_adapter_wasms().unwrap();
        let b = Bootstrap::new();
        b.configure_default_feed();
        b.submit_and_execute(
            &b.admin,
            GovernanceAction::ConfigureBreakers(b.asset_btc.clone(), 0, 16),
        );
        for (timestamp, price) in [
            (100_u64, 1_000_000_000_i128),
            (101, 1_100_000_000),
            (110, 1_200_000_000),
            (111, 1_300_000_000),
        ] {
            ledger::advance_secs(&b.env, timestamp - b.env.ledger().timestamp());
            b.push_upstream_price(&b.asset_btc, price, timestamp);
            b.refresh_one(&b.asset_btc);
        }
        b.env.cost_estimate().budget().reset_default();
        let old_hash = b.env.deployer().upload_contract_wasm(old_wasm.as_slice());
        b.env.cost_estimate().budget().reset_default();
        let new_hash = b.env.deployer().upload_contract_wasm(new_wasm.as_slice());
        assert_ne!(old_hash, new_hash);
        for decommissioned in [false, true] {
            b.env.cost_estimate().budget().reset_default();
            let id = b.env.register(
                old_wasm.as_slice(),
                (
                    &b.admin,
                    &b.runtime_id,
                    &b.asset_btc,
                    &8_u32,
                    &10_u32,
                    &b.base_usd,
                ),
            );
            let adapter = Sep40AdapterClient::new(&b.env, &id);
            assert_eq!(id.executable(), Some(Executable::Wasm(old_hash.clone())));
            authorize_adapter(
                &b.env,
                &b.admin,
                &id,
                "set_decimals",
                (7_u32,).into_val(&b.env),
            );
            adapter.set_decimals(&7);
            let owner = Address::generate(&b.env);
            let deadline = b.env.ledger().max_live_until_ledger();
            authorize_adapter(
                &b.env,
                &b.admin,
                &id,
                "transfer_ownership",
                (&owner, deadline).into_val(&b.env),
            );
            adapter.transfer_ownership(&owner, &deadline);
            authorize_adapter(&b.env, &owner, &id, "accept_ownership", Vec::new(&b.env));
            adapter.accept_ownership();
            if decommissioned {
                authorize_adapter(&b.env, &owner, &id, "decommission", Vec::new(&b.env));
                adapter.decommission();
            }
            let config_before = adapter.config().unwrap().to_xdr(&b.env);
            assert_eq!(adapter.get_owner(), Some(owner.clone()));
            assert_eq!(adapter.lastprice(&b.asset_btc).is_none(), decommissioned);
            if !decommissioned {
                // The exact released executable exhibits both historical defects.
                assert_eq!(adapter.price(&b.asset_btc, &109), None);
                assert_eq!(adapter.prices(&b.asset_btc, &2).unwrap().len(), 1);
                assert_eq!(
                    adapter
                        .prices(&b.asset_btc, &3)
                        .unwrap()
                        .get(0)
                        .unwrap()
                        .timestamp,
                    100
                );
            }
            b.env.cost_estimate().budget().reset_default();
            authorize_adapter(
                &b.env,
                &owner,
                &id,
                "upgrade",
                (&new_hash, &owner).into_val(&b.env),
            );
            adapter.upgrade(&new_hash, &owner);
            assert_eq!(id.executable(), Some(Executable::Wasm(new_hash.clone())));
            assert_eq!(adapter.config().unwrap().to_xdr(&b.env), config_before);
            assert_eq!(adapter.get_owner(), Some(owner.clone()));
            assert_eq!(adapter.base(), b.base_usd);
            assert_eq!(adapter.decimals(), 7);
            assert_eq!(adapter.resolution(), 10);
            if decommissioned {
                assert!(adapter.assets().is_empty());
                assert_eq!(adapter.lastprice(&b.asset_btc), None);
                assert_eq!(adapter.prices(&b.asset_btc, &2), None);
                assert_eq!(adapter.price(&b.asset_btc, &109), None);
                authorize_adapter(
                    &b.env,
                    &owner,
                    &id,
                    "set_decimals",
                    (8_u32,).into_val(&b.env),
                );
                assert_eq!(
                    adapter.try_set_decimals(&8),
                    Err(Ok(ContractError::InvalidInput))
                );
            } else {
                let newest = PriceData {
                    price: 130_000_000,
                    timestamp: 110,
                };
                let previous = PriceData {
                    price: 110_000_000,
                    timestamp: 100,
                };
                assert_eq!(
                    adapter.prices(&b.asset_btc, &2),
                    Some(soroban_sdk::vec![&b.env, newest.clone(), previous.clone()])
                );
                assert_eq!(adapter.price(&b.asset_btc, &109), Some(previous));
                assert_eq!(adapter.lastprice(&b.asset_btc), Some(newest));
                assert_eq!(
                    adapter.assets(),
                    soroban_sdk::vec![&b.env, b.asset_btc.clone()]
                );
            }
        }
    }
}

#[test]
fn upgrade_proposal_with_zero_hash_is_rejected_at_create_time() {
    let b = Bootstrap::new();
    let zero = BytesN::<32>::from_array(&b.env, &[0_u8; 32]);
    // `validate_action` runs in `create_proposal`, so the create call is the
    // error site — the proposal never makes it to the pending set.
    let next_id = b.governance.next_proposal_id();
    let result =
        b.governance
            .try_create_proposal(&b.admin, &next_id, &GovernanceAction::Upgrade(zero), &0);
    assert!(result.is_err(), "zero wasm hash must be rejected");
}

#[test]
#[should_panic]
fn direct_runtime_upgrade_by_non_owner_panics() {
    let b = Bootstrap::new();
    let rando = Address::generate(&b.env);
    let dummy_hash = BytesN::<32>::random(&b.env);
    // try_upgrade is the panic-returning try variant; the wrapper will
    // propagate the inner error in soroban-sdk testutils, but
    // `unwrap_err`-style introspection isn't reliable for owner-check
    // panics. Use `should_panic` as the simplest verification.
    b.runtime.upgrade(&dummy_hash, &rando);
}
