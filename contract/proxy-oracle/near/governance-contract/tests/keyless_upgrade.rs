//! Governance-driven upgrades need no access key on the upgraded account, and the wasm they carry is
//! no longer bounded by what a receipt may log.
//!
//! Every access key is deleted from both accounts before any upgrade runs, so the only thing left
//! that can replace their code is the code itself: `SelfUpgrade` on the governance contract and the
//! owner-gated `admin_upgrade` on the oracle, each a `Promise::deploy_contract` against
//! `current_account_id`.
//!
//! Payload size is what these tests are really about. A proposal used to be emitted in full by its
//! creation event, capping it at the node's per-receipt [`MAX_TOTAL_LOG_LENGTH`];
//! `Event::Created`/`Executed` now carry ids and method names only, which
//! [`assert_payload_stayed_out_of_the_logs`] pins.
//!
//! Nothing about the wasm's size blocks an upgrade now — a whole contract goes through inline on
//! both paths. What is left is a *gas budget* question, and only for the target path, where the
//! reservations compound across two receipts: see
//! [`inline_target_upgrade_lands_with_enough_gas`] for what it costs and
//! [`inline_target_upgrade_at_default_gas_reverts_whole`] for why the defaults miss.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use anyhow::Result;
use near_api::types::transaction::result::ExecutionSuccess;
use near_api::types::AccountId;
use near_sdk::json_types::Base64VecU8;
use near_sdk::Gas;
use rstest::rstest;
use serde_json::json;
use templar_common::upgrade::UpgradeSource;
use templar_gateway_testing::{harness, wasm, SandboxHarness};
use templar_proxy_oracle_near_governance_common::{target, Operation};

use common::{
    code_hash, deploy_global_contract, governed, revoke_all_access_keys, self_upgrade, view,
    Encoding, Governed, GATEWAY_GAS, PRICE_ID,
};

/// The node's per-receipt cap on total log output — the ceiling a proposal used to be measured
/// against, back when its creation event carried the operation body.
const MAX_TOTAL_LOG_LENGTH: usize = 16 * 1024;

/// A metadata-only `Event` is a few hundred bytes. Any leak of the payload — even base64'd, even
/// truncated to fit — clears this by orders of magnitude.
const MAX_EVENT_LOG_BYTES: usize = 1024;

fn assert_payload_stayed_out_of_the_logs(outcome: &ExecutionSuccess, label: &str) {
    let logged: usize = outcome.logs().iter().map(|log| log.len()).sum();
    assert!(
        logged <= MAX_EVENT_LOG_BYTES,
        "{label} logged {logged} bytes; events must carry proposal metadata, not the payload: {:#?}",
        outcome.logs(),
    );
}

/// The wasm under test, guarded to be genuinely in the size regime the old event-carrying proposal
/// could not reach.
fn oversized_payload(code: Vec<u8>, label: &str) -> Base64VecU8 {
    assert!(
        code.len() > MAX_TOTAL_LOG_LENGTH,
        "{label} is {} bytes, too small to prove anything about the log cap",
        code.len(),
    );
    Base64VecU8(code)
}

/// Redeploying identical bytes leaves the code hash untouched, so park governance on a global
/// contract of its own bytes first: the inline deploy of those same bytes is then visible as
/// `contract_state` moving back to local code. Returns the parked `contract_state`.
async fn park_on_global(governed: &mut Governed, code: Vec<u8>) -> Result<String> {
    let published = deploy_global_contract(&governed.network, &governed.admin, code).await?;
    governed
        .govern(self_upgrade(UpgradeSource::GlobalHash(published)))
        .await?;
    code_hash(&governed.network, &governed.governance).await
}

/// `admin_upgrade` at its default `GAS_FOR_ADMIN_UPGRADE` (280 Tgas) forwarded to the oracle.
fn admin_upgrade(source: UpgradeSource) -> Operation {
    Operation::TargetFunctionCall(
        target::admin_upgrade(source, Base64VecU8(Vec::new()), None)
            .expect("admin_upgrade args serialize"),
    )
}

fn admin_upgrade_with_gas(source: UpgradeSource, gas: Gas) -> Operation {
    Operation::TargetFunctionCall(
        target::admin_upgrade(source, Base64VecU8(Vec::new()), Some(gas))
            .expect("admin_upgrade args serialize"),
    )
}

/// A whole contract deployed over itself from inside a proposal, on an account nothing holds a key
/// to. The payload is ~20× [`MAX_TOTAL_LOG_LENGTH`], which is the point.
#[rstest]
#[tokio::test]
async fn keyless_governance_self_upgrades_carrying_its_whole_wasm(
    #[future(awt)] harness: SandboxHarness,
) -> Result<()> {
    let mut governed = governed(&harness).await?;
    revoke_all_access_keys(&harness, &governed.governance).await?;

    let code = wasm::proxy_governance().await.to_vec();
    let code_len = code.len();
    let parked = park_on_global(&mut governed, code.clone()).await?;

    let operation = self_upgrade(UpgradeSource::Code(oversized_payload(
        code,
        "the governance wasm",
    )));
    let (id, created) = governed.propose(operation, Encoding::Borsh).await?;
    let executed = governed.execute(id).await?;

    assert_payload_stayed_out_of_the_logs(&created, "create_proposal_borsh");
    assert_payload_stayed_out_of_the_logs(&executed, "execute_proposal");
    assert_ne!(
        parked,
        code_hash(&governed.network, &governed.governance).await?,
        "governance should now run the deployed blob, not the global contract it was parked on"
    );

    let version = harness.contract_state_version(&governed.governance).await?;
    assert_eq!((version.stored, version.target), (1, 1));
    assert!(!version.needs_migration);

    // Still governing after replacing its own code — the one thing a code hash cannot show.
    governed
        .govern(Operation::TargetFunctionCall(target::admin_set_proxy(
            PRICE_ID, None, None,
        )?))
        .await?;
    assert_eq!(governed.proxy().await?, None);

    // The margin that decides how much bigger this contract can get. `execute_proposal` reads the
    // whole blob back out of storage, and whatever it burns doing so has to leave `GAS_FOR_MIGRATE`
    // (250 Tgas) of the attached `GATEWAY_GAS` free for the migrate batched onto the self-deploy.
    // Unlike the target path there is only one receipt here, so one reservation has to fit, not two.
    let executing = &executed.receipt_outcomes()[0];
    println!(
        "self-upgrade over {code_len} bytes of wasm: execute_proposal burnt {}, {} total",
        executing.gas_burnt, executed.total_gas_burnt,
    );
    Ok(())
}

/// The oracle upgraded by its governance, with neither account holding a key. Pinned by hash, which
/// is what keeps this inside the gateway's budget — see the two inline tests below.
#[rstest]
#[tokio::test]
async fn keyless_oracle_upgrades_through_its_governance(
    #[future(awt)] harness: SandboxHarness,
) -> Result<()> {
    let mut governed = governed(&harness).await?;
    revoke_all_access_keys(&harness, &governed.oracle).await?;
    revoke_all_access_keys(&harness, &governed.governance).await?;

    let before = code_hash(&governed.network, &governed.oracle).await?;
    let published = deploy_global_contract(
        &governed.network,
        &governed.admin,
        wasm::proxy_oracle().await.to_vec(),
    )
    .await?;
    let executed = governed
        .govern(admin_upgrade(UpgradeSource::GlobalHash(published)))
        .await?;

    assert_payload_stayed_out_of_the_logs(&executed, "execute_proposal");
    assert_ne!(
        before,
        code_hash(&governed.network, &governed.oracle).await?,
        "the oracle should now run the global contract, not its own local code"
    );

    let version = harness.contract_state_version(&governed.oracle).await?;
    assert_eq!((version.stored, version.target), (1, 1));
    assert!(!version.needs_migration);
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
    assert_eq!(
        governed.proxy().await?,
        Some(common::proxy()),
        "the seeded proxy should have survived the upgrade"
    );
    Ok(())
}

/// Enough to clear both legs of the chain in [`inline_target_upgrade_lands_with_enough_gas`], with
/// headroom. Both are ordinary caller-side values, not protocol ceilings.
const AMPLE_ATTACHED_GAS: Gas = Gas::from_tgas(500);
const AMPLE_FORWARDED_GAS: Gas = Gas::from_tgas(400);

/// Carried inline, the oracle wasm upgrades the oracle just fine — the payload size is not the
/// obstacle. What it costs is gas, and the reservations are the reason the defaults miss:
/// `execute_proposal` burns ~61 Tgas reading the proposal back, the oracle burns ~59 Tgas parsing
/// the blob out of its base64-in-JSON args, and only then does `admin_upgrade` reserve
/// `GAS_FOR_MIGRATE` (250 Tgas) for the migrate batched onto its self-deploy. Actual work is
/// ~155 Tgas; it is the reservations that have to fit, and they compound down the chain.
#[rstest]
#[tokio::test]
async fn inline_target_upgrade_lands_with_enough_gas(
    #[future(awt)] harness: SandboxHarness,
) -> Result<()> {
    let mut governed = governed(&harness).await?;
    revoke_all_access_keys(&harness, &governed.oracle).await?;
    revoke_all_access_keys(&harness, &governed.governance).await?;

    // Park the oracle on a global contract of its own bytes, so redeploying those same bytes inline
    // is visible as `contract_state` moving back to local code.
    let code = wasm::proxy_oracle().await.to_vec();
    let published =
        deploy_global_contract(&governed.network, &governed.admin, code.clone()).await?;
    governed
        .govern(admin_upgrade(UpgradeSource::GlobalHash(published)))
        .await?;
    let parked = code_hash(&governed.network, &governed.oracle).await?;

    let operation = admin_upgrade_with_gas(
        UpgradeSource::Code(oversized_payload(code, "the proxy oracle wasm")),
        AMPLE_FORWARDED_GAS,
    );
    let (id, created) = governed.propose(operation, Encoding::Borsh).await?;
    let executed = governed
        .try_execute_with_gas(id, AMPLE_ATTACHED_GAS)
        .await?
        .into_result()
        .map_err(|error| anyhow::anyhow!("inline target upgrade failed: {error}"))?;

    assert_payload_stayed_out_of_the_logs(&created, "create_proposal_borsh");
    assert_payload_stayed_out_of_the_logs(&executed, "execute_proposal");
    assert!(
        executed.receipt_failures().is_empty(),
        "the upgrade dispatches on a detached promise, so its failures are receipt-level: {:#?}",
        executed.receipt_failures(),
    );
    assert_ne!(
        parked,
        code_hash(&governed.network, &governed.oracle).await?,
        "the oracle should now run the deployed blob, not the global contract it was parked on"
    );

    let version = harness.contract_state_version(&governed.oracle).await?;
    assert_eq!((version.stored, version.target), (1, 1));
    assert!(!version.needs_migration);
    assert_eq!(
        governed.proxy().await?,
        Some(common::proxy()),
        "the seeded proxy should have survived the upgrade"
    );
    Ok(())
}

/// The same upgrade at the defaults does not fit, so this path is unreachable through the gateway
/// today: `GAS_FOR_ADMIN_UPGRADE` forwards a fixed 280 Tgas, ~29 Tgas short of the ~309 the oracle
/// needs to parse the blob and still reserve `GAS_FOR_MIGRATE`. Attaching more to the transaction
/// cannot help — the forwarded amount is what binds, and only `target::admin_upgrade`'s `gas`
/// override raises it. `GlobalHash` sidesteps the whole chain by keeping the proposal small.
///
/// That it *reverts whole* is the part worth pinning: nothing bricked, nothing half-applied, and the
/// proposal stays pending for a later execution with a bigger budget.
#[rstest]
#[tokio::test]
async fn inline_target_upgrade_at_default_gas_reverts_whole(
    #[future(awt)] harness: SandboxHarness,
) -> Result<()> {
    let mut governed = governed(&harness).await?;
    let before = code_hash(&governed.network, &governed.oracle).await?;

    let operation = admin_upgrade(UpgradeSource::Code(oversized_payload(
        wasm::proxy_oracle().await.to_vec(),
        "the proxy oracle wasm",
    )));
    let (id, _) = governed.propose(operation.clone(), Encoding::Borsh).await?;

    let failure = governed
        .try_execute_with_gas(id, GATEWAY_GAS)
        .await?
        .into_result()
        .expect_err("the default forwarded gas should not fit an inline oracle wasm")
        .to_string();
    assert!(
        failure.contains("Exceeded the prepaid gas"),
        "expected a gas rejection, got: {failure}"
    );

    assert_eq!(
        governed.proposal(id).await?.map(|p| p.operation),
        Some(operation),
        "the reverted execution should have left the proposal pending and intact"
    );
    assert_eq!(
        before,
        code_hash(&governed.network, &governed.oracle).await?,
        "the oracle should still run the code it had before the failed upgrade"
    );
    assert_eq!(
        governed.proxy().await?,
        Some(common::proxy()),
        "the oracle should still answer for its seeded proxy"
    );

    // And governance itself is unharmed: it can still drive the oracle.
    governed
        .govern(Operation::TargetFunctionCall(target::admin_set_proxy(
            PRICE_ID, None, None,
        )?))
        .await?;
    assert_eq!(governed.proxy().await?, None);
    Ok(())
}
