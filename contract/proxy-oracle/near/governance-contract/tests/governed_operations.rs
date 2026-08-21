//! Every operation the governance contract can execute, driven for real against a deployed proxy
//! oracle.
//!
//! `execute_proposal` dispatches on a detached promise, so a wrong method name or arg field fails on
//! its own receipt while the governance transaction still succeeds — hence assertions on the
//! oracle's observable state rather than on transaction status.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use anyhow::Result;
use near_api::types::AccountId;
use near_sdk::json_types::Base64VecU8;
use rstest::rstest;
use serde_json::json;
use templar_common::{upgrade::UpgradeSource, Decimal, Nanoseconds};
use templar_gateway_testing::{harness, wasm, SandboxHarness};
use templar_proxy_oracle_kernel::proxy::circuit_breaker::{
    CircuitBreaker, CircuitBreakerSetConfig, CircuitBreakerStatus, StepwiseChange,
};
use templar_proxy_oracle_near_governance_common::{target, Operation, ReflexiveOperation, Role};

use common::{code_hash, deploy_global_contract, governed, self_upgrade, view, PRICE_ID};

/// `add` requires `breaker_id == next_id`, which starts at zero and only ever increments.
const BREAKER: u32 = 0;

fn breaker() -> CircuitBreaker {
    CircuitBreaker::StepwiseChange(StepwiseChange {
        max_relative_change: Decimal::ONE_HALF,
    })
}

/// Each distinct from the default it overwrites, so a silent no-op cannot read as a success.
const REARM_DELAY: Nanoseconds = Nanoseconds::from_secs(4_242);
const SAMPLE_INTERVAL: Nanoseconds = Nanoseconds::from_secs(60);

/// Every proxy/circuit-breaker builder in `target.rs`, each proved by the state change it makes on
/// the oracle. `admin_upgrade` is the one target method not here — it has no view to read back, so
/// it gets its own test below.
#[rstest]
#[tokio::test]
async fn every_target_builder_drives_its_admin_method(
    #[future(awt)] harness: SandboxHarness,
) -> Result<()> {
    let mut governed = governed(&harness).await?;

    governed
        .govern(Operation::TargetFunctionCall(
            target::admin_configure_circuit_breakers(
                PRICE_ID,
                CircuitBreakerSetConfig {
                    sample_interval_ns: SAMPLE_INTERVAL,
                    history_len: 8,
                },
                None,
            )?,
        ))
        .await?;
    assert_eq!(
        governed.breakers().await?.sample_interval_ns(),
        SAMPLE_INTERVAL,
        "admin_configure_circuit_breakers"
    );

    governed
        .govern(Operation::TargetFunctionCall(
            target::admin_add_circuit_breaker(PRICE_ID, BREAKER, breaker(), None)?,
        ))
        .await?;
    assert!(
        governed.breakers().await?.breakers().contains_key(&BREAKER),
        "admin_add_circuit_breaker"
    );

    governed
        .govern(Operation::TargetFunctionCall(
            target::admin_set_manual_trip(PRICE_ID, true, Some(vec![0x01, 0x02]), None)?,
        ))
        .await?;
    assert!(
        governed.breakers().await?.is_manually_tripped(),
        "admin_set_manual_trip"
    );

    governed
        .govern(Operation::TargetFunctionCall(target::admin_rearm(
            PRICE_ID,
            BREAKER,
            REARM_DELAY,
            None,
        )?))
        .await?;
    assert!(
        matches!(
            governed.breakers().await?.breakers()[&BREAKER].status,
            CircuitBreakerStatus::ArmedAfter { timestamp_ns }
                if timestamp_ns.as_ns() >= REARM_DELAY.as_ns()
        ),
        "admin_rearm"
    );

    governed
        .govern(Operation::TargetFunctionCall(target::admin_set_enforced(
            PRICE_ID, BREAKER, false, None,
        )?))
        .await?;
    assert!(
        !governed.breakers().await?.breakers()[&BREAKER].is_enforced,
        "admin_set_enforced"
    );

    governed
        .govern(Operation::TargetFunctionCall(
            target::admin_remove_circuit_breaker(PRICE_ID, BREAKER, None)?,
        ))
        .await?;
    assert!(
        !governed.breakers().await?.breakers().contains_key(&BREAKER),
        "admin_remove_circuit_breaker"
    );

    // Last: clearing the proxy takes the circuit-breaker set with it.
    governed
        .govern(Operation::TargetFunctionCall(target::admin_set_proxy(
            PRICE_ID, None, None,
        )?))
        .await?;
    assert_eq!(governed.proxy().await?, None, "admin_set_proxy");
    Ok(())
}

/// `admin_upgrade` writes no readable state, and redeploying the current wasm leaves the code hash
/// unchanged — so it is proved by `contract_state` moving from local code to a global contract of
/// those same bytes. The released `0.3.0` would be the obvious different-bytes target, but its
/// `migrate` predates the empty-`migrate_args` no-op and rejects them.
#[rstest]
#[tokio::test]
async fn admin_upgrade_replaces_the_oracle_code(
    #[future(awt)] harness: SandboxHarness,
) -> Result<()> {
    let mut governed = governed(&harness).await?;
    let before = code_hash(&governed.network, &governed.oracle).await?;
    let published = deploy_global_contract(
        &governed.network,
        &governed.admin,
        wasm::proxy_oracle().await.to_vec(),
    )
    .await?;

    governed
        .govern(Operation::TargetFunctionCall(target::admin_upgrade(
            UpgradeSource::GlobalHash(published),
            Base64VecU8(Vec::new()),
            None,
        )?))
        .await?;

    assert_ne!(
        before,
        code_hash(&governed.network, &governed.oracle).await?,
        "oracle code should have been replaced"
    );
    assert_eq!(
        view::<Option<AccountId>>(
            &governed.network,
            &governed.oracle,
            "own_get_owner",
            json!({})
        )
        .await?,
        Some(governed.governance.clone()),
        "the upgraded oracle should still be owned by its governance"
    );
    Ok(())
}

#[rstest]
#[tokio::test]
async fn borsh_entrypoint_drives_target_and_reflexive_operations(
    #[future(awt)] harness: SandboxHarness,
) -> Result<()> {
    let mut governed = governed(&harness).await?;

    governed
        .govern_borsh(Operation::TargetFunctionCall(
            target::admin_set_manual_trip(PRICE_ID, true, None, None)?,
        ))
        .await?;
    assert!(
        governed.breakers().await?.is_manually_tripped(),
        "borsh-created target call should have tripped the breaker"
    );

    let operator: AccountId = "operator.near".parse()?;
    governed
        .govern_borsh(Operation::Reflexive(ReflexiveOperation::SetRole {
            account_id: operator.clone(),
            role: Role::ManualTripper,
            set: true,
        }))
        .await?;
    assert!(
        view::<bool>(
            &governed.network,
            &governed.governance,
            "has_role",
            json!({ "account_id": operator, "role": "ManualTripper" }),
        )
        .await?,
        "borsh-created reflexive op should have granted the role"
    );
    Ok(())
}

/// The path every future governance upgrade takes, which `upgrade_ordering` cannot cover: the v0
/// contract has no `SelfUpgrade` and upgrades by full-access key instead. Both `UpgradeSource`
/// variants run here — identical redeployed bytes leave the code hash unchanged, so only the switch
/// to a global contract can show the deploy action landing.
#[rstest]
#[tokio::test]
async fn self_upgrade_redeploys_governance_and_keeps_it_working(
    #[future(awt)] harness: SandboxHarness,
) -> Result<()> {
    let mut governed = governed(&harness).await?;

    governed
        .govern(self_upgrade(UpgradeSource::Code(Base64VecU8(
            wasm::proxy_governance().await.to_vec(),
        ))))
        .await?;

    // Still governing after replacing its own code, which a redeployed hash cannot show.
    governed
        .govern(Operation::TargetFunctionCall(target::admin_set_proxy(
            PRICE_ID, None, None,
        )?))
        .await?;
    assert_eq!(governed.proxy().await?, None);

    let before = code_hash(&governed.network, &governed.governance).await?;
    let published = deploy_global_contract(
        &governed.network,
        &governed.admin,
        wasm::proxy_governance().await.to_vec(),
    )
    .await?;
    governed
        .govern(self_upgrade(UpgradeSource::GlobalHash(published)))
        .await?;
    assert_ne!(
        before,
        code_hash(&governed.network, &governed.governance).await?,
        "governance should now run the global contract, not its own local code"
    );
    Ok(())
}
