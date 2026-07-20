//! Shared helpers for the vault integration tests (`happy_path`, `governance`,
//! `callback_failure`). Kept in one place so the harness-driving logic and the
//! address derivation don't drift between test files.
//!
//! Each test binary pulls in this module via `mod common;` and uses only the
//! helpers it needs, so allow the unused ones per-binary.
#![allow(dead_code)]

use anyhow::Result;
use templar_common::{
    interest_rate_strategy::InterestRateStrategy, market::MarketConfiguration, Decimal,
};
use templar_gateway_testing::{DeployedVault, ManagedAccountId, SandboxHarness};
use templar_vault_kernel::Address;

/// Zero-interest borrow strategy — the common market customization these tests use.
#[allow(clippy::unwrap_used)] // infallible zero/zero strategy, in a non-`#[test]` helper
pub fn zero_interest(c: &mut MarketConfiguration) {
    c.borrow_interest_rate_strategy =
        InterestRateStrategy::linear(Decimal::ZERO, Decimal::ZERO).unwrap();
}

/// Drive market yield harvesting until the vault's supply position is fully
/// activated (no incoming deposits pending).
pub async fn harvest(harness: &SandboxHarness, vault: &DeployedVault) -> Result<()> {
    let vault_account = ManagedAccountId(vault.vault_id.clone());
    while let Some(position) = harness
        .get_supply_position(&vault.market, &vault.vault_id)
        .await?
    {
        if position.get_deposit().incoming.is_empty() {
            break;
        }
        harness
            .harvest_yield(&vault_account, &vault.market, None)
            .await?;
    }
    Ok(())
}

/// The kernel address the vault derives for an account, computed by the
/// contract's own [`account_id_to_address`](templar_vault_contract::account_id_to_address)
/// so the test can never diverge from the on-chain mapping.
#[allow(clippy::expect_used)] // ids originate from real accounts, so the parse is infallible
pub fn account_to_kernel_address(account: &near_api::types::AccountId) -> Address {
    let account: near_sdk::AccountId = account
        .as_str()
        .parse()
        .expect("harness account ids are valid near_sdk account ids");
    templar_vault_contract::account_id_to_address(&account)
}
