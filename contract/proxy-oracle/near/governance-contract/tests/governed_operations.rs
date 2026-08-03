//! Every operation the governance contract can execute, driven for real against a deployed proxy
//! oracle.
//!
//! `execute_proposal` dispatches on a detached promise, so a wrong method name or arg field fails on
//! its own receipt while the governance transaction still succeeds — hence assertions on the
//! oracle's observable state rather than on transaction status.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use anyhow::{Context, Result};
use near_api::types::AccountId;
use near_api::NetworkConfig;
use near_sdk::json_types::Base64VecU8;
use near_sdk::serde::Serialize;
use rstest::rstest;
use serde_json::json;
use templar_common::{oracle::pyth::PriceIdentifier, upgrade::UpgradeSource, Decimal, Nanoseconds};
use templar_gateway_testing::{harness, wasm, SandboxHarness};
use templar_proxy_oracle_kernel::proxy::{
    circuit_breaker::{
        AcceptedHistorySource, CircuitBreaker, CircuitBreakerSet, CircuitBreakerSetConfig,
        CircuitBreakerStatus, StepwiseChange,
    },
    FreshnessFilter, Proxy,
};
use templar_proxy_oracle_near_common::{input::Source, request::OracleRequest};
use templar_proxy_oracle_near_governance_common::{target, Operation, ReflexiveOperation, Role};

use common::{
    call, call_borsh, code_hash, deploy_global_contract, self_upgrade, view, CreateProposalArgs,
    ProposalIdArgs, ONE_YOCTO,
};

/// `add` requires `breaker_id == next_id`, which starts at zero and only ever increments.
const BREAKER: u32 = 0;

const PRICE_ID: PriceIdentifier = PriceIdentifier([0xaa; 32]);

#[derive(Serialize)]
#[serde(crate = "near_sdk::serde")]
struct PriceIdArgs {
    id: PriceIdentifier,
}

#[derive(Clone, Copy)]
enum Encoding {
    Json,
    Borsh,
}

impl Encoding {
    async fn create_proposal(
        self,
        governed: &Governed,
        id: u32,
        operation: Operation,
    ) -> Result<()> {
        match self {
            Self::Json => call(
                &governed.network,
                &governed.governance,
                "create_proposal",
                CreateProposalArgs {
                    id,
                    operation,
                    requested_ttl: Nanoseconds::zero(),
                },
                &governed.admin,
                ONE_YOCTO,
            )
            .await
            .map(|_| ()),
            Self::Borsh => call_borsh(
                &governed.network,
                &governed.governance,
                "create_proposal_borsh",
                CreateProposalArgs {
                    id,
                    operation,
                    requested_ttl: Nanoseconds::zero(),
                },
                &governed.admin,
                ONE_YOCTO,
            )
            .await
            .map(|_| ()),
        }
    }
}

/// An oracle owned by a governance contract, with a proxy and one circuit breaker already in place.
struct Governed {
    network: NetworkConfig,
    oracle: AccountId,
    governance: AccountId,
    admin: AccountId,
    next_id: u32,
}

impl Governed {
    /// Run `operation` through the full create → execute path as the admin. Every TTL is zero
    /// (`deploy_governance_contract` installs a uniform zero policy), so it matures immediately.
    async fn govern(&mut self, operation: Operation) -> Result<()> {
        self.govern_with(operation, Encoding::Json).await
    }

    async fn govern_borsh(&mut self, operation: Operation) -> Result<()> {
        self.govern_with(operation, Encoding::Borsh).await
    }

    async fn govern_with(&mut self, operation: Operation, encoding: Encoding) -> Result<()> {
        let id = self.next_id;
        self.next_id += 1;

        encoding.create_proposal(self, id, operation).await?;
        call(
            &self.network,
            &self.governance,
            "execute_proposal",
            ProposalIdArgs { id },
            &self.admin,
            ONE_YOCTO,
        )
        .await
        .map(|_| ())
    }

    async fn proxy(&self) -> Result<Option<Proxy<Source>>> {
        view(
            &self.network,
            &self.oracle,
            "get_proxy",
            PriceIdArgs { id: PRICE_ID },
        )
        .await
    }

    async fn breakers(&self) -> Result<CircuitBreakerSet> {
        view::<Option<CircuitBreakerSet>>(
            &self.network,
            &self.oracle,
            "get_proxy_circuit_breaker_set",
            PriceIdArgs { id: PRICE_ID },
        )
        .await?
        .context("the seeded proxy is gone")
    }
}

fn proxy() -> Proxy<Source> {
    Proxy::median_low(
        [OracleRequest::pyth("pyth.near".parse().unwrap(), PriceIdentifier([0xbb; 32])).into()],
        FreshnessFilter::empty(),
    )
}

fn breaker() -> CircuitBreaker {
    CircuitBreaker::StepwiseChange(StepwiseChange {
        max_relative_change: Decimal::ONE_HALF,
    })
}

/// Deploy the oracle with a proxy in place, then hand it to a governance contract. The proxy is
/// seeded while the oracle still owns itself — one direct call instead of a governance round-trip.
async fn governed(harness: &SandboxHarness) -> Result<Governed> {
    let oracle = harness.deploy_proxy_oracle().await?;
    let admin = harness.create_user("admin").await?.0;
    harness
        .admin_set_proxy(oracle.clone(), PRICE_ID, Some(proxy()))
        .await?;
    let governance = harness
        .deploy_governance_contract(oracle.clone(), admin.clone())
        .await?;

    Ok(Governed {
        network: harness.network.clone(),
        oracle,
        governance,
        admin,
        // `deploy_governance_contract` consumes id 0 for the ownership handover.
        next_id: 1,
    })
}

/// Each distinct from the default it overwrites, so a silent no-op cannot read as a success.
const REARMED_AT: Nanoseconds = Nanoseconds::from_secs(4_242);
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
            REARMED_AT,
            AcceptedHistorySource::Empty,
            None,
        )?))
        .await?;
    assert_eq!(
        governed.breakers().await?.breakers()[&BREAKER].status,
        CircuitBreakerStatus::ArmedAfter {
            timestamp_ns: REARMED_AT
        },
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
