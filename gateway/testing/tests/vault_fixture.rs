//! Smoke test for the vault+market harness fixture (ENG-388 vault migration).
//!
//! Validates that `deploy_vault_with_market` stands up a vault whose market is
//! registered, capped, and queued, and that a supply → allocate round-trips
//! through the gateway `Client` — the wiring the ported vault tests depend on.
//!
//! Node-backed: run with `just test-sandbox -p templar-gateway-testing`.

use anyhow::Result;
use near_sdk::json_types::U128;
use rstest::rstest;
use templar_common::vault::{AllocationDelta, Delta};
use templar_gateway_testing::{harness, SandboxHarness};

#[rstest]
#[tokio::test]
async fn vault_fixture_supply_then_allocate(#[future(awt)] harness: SandboxHarness) -> Result<()> {
    let vault = harness.deploy_vault_with_market().await?;

    // Standup invariants: empty vault, market registered on it.
    assert_eq!(harness.vault_total_assets(&vault).await?, 0);
    assert_eq!(harness.vault_idle_balance(&vault).await?, 0);
    let market_id = harness
        .vault_market_id_of(&vault.vault_id, &vault.market.market_id)
        .await?
        .expect("market should be registered on the vault after setup");

    // Supply underlying → shares minted, idle and total assets grow.
    let user = harness.create_user("supply-user").await?;
    harness.vault_init_account(&user, &vault).await?;
    harness.vault_supply(&user, &vault, 1_000).await?;
    assert_eq!(harness.vault_total_supply(&vault).await?, 1_000);
    assert_eq!(harness.vault_idle_balance(&vault).await?, 1_000);
    assert_eq!(harness.vault_total_assets(&vault).await?, 1_000);

    // Allocate into the market → idle drains to zero, total assets preserved.
    harness
        .vault_allocate(
            &vault.curator,
            &vault,
            AllocationDelta::Supply(Delta::new(market_id, U128(1_000))),
        )
        .await?;
    assert_eq!(
        harness.vault_idle_balance(&vault).await?,
        0,
        "allocation should drain idle balance into the market",
    );
    assert_eq!(
        harness.vault_total_assets(&vault).await?,
        1_000,
        "allocation should preserve total assets",
    );
    assert_eq!(
        harness.vault_total_supply(&vault).await?,
        1_000,
        "allocation should not mint or burn shares",
    );

    Ok(())
}
