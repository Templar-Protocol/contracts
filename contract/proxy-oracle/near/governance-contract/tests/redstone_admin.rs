//! Actual released-WASM bootstrap and keyless RedStone administration through generic governance.
mod common;
#[path = "redstone_admin/governed.rs"]
mod governed;
#[path = "redstone_admin/legacy.rs"]
mod legacy;

use anyhow::Result;
use near_api::types::AccountId;
use near_sdk::json_types::Base64VecU8;
use serde_json::{json, Value};
use templar_gateway_testing::SandboxHarness;

#[allow(
    clippy::expect_used,
    reason = "a corrupt checked-in signed fixture must fail the proof"
)]
fn payload() -> Base64VecU8 {
    Base64VecU8(
        hex::decode(include_str!("../../../../redstone-adapter/tests/fixtures/eth_btc.hex").trim())
            .expect("checked-in signed payload is hex"),
    )
}

async fn snapshot(harness: &SandboxHarness, adapter: &AccountId) -> Result<Value> {
    let mut roles = Vec::new();
    for role in ["ModifyRoles", "TrustedUpdater"] {
        let mut members: Vec<AccountId> = common::view(
            &harness.network,
            adapter,
            "list_role",
            json!({"role": role}),
        )
        .await?;
        members.sort();
        roles.push(members);
    }
    Ok(json!({
        "config": common::view::<Value>(&harness.network, adapter, "get_config", json!({})).await?,
        "feeds": common::view::<Value>(&harness.network, adapter, "read_price_data", json!({"feed_ids": ["ETH", "BTC"]})).await?,
        "modify_roles": roles[0],
        "trusted_updaters": roles[1],
    }))
}

async fn ownership(harness: &SandboxHarness, adapter: &AccountId) -> Result<Value> {
    Ok(json!({
        "owner": common::view::<Value>(&harness.network, adapter, "own_get_owner", json!({})).await?,
        "proposed": common::view::<Value>(&harness.network, adapter, "own_get_proposed_owner", json!({})).await?,
    }))
}
