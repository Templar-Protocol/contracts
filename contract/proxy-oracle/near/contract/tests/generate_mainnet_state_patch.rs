#![allow(clippy::unwrap_used)]

use std::fs;

use near_sdk::base64::prelude::*;
use near_sdk::{serde::Deserialize, serde_json};

#[path = "support/migration_fixture.rs"]
mod fixture;

#[derive(Deserialize)]
#[serde(crate = "near_sdk::serde")]
struct StateEntryJson {
    key: String,
    value: String,
}

#[test]
#[ignore = "fixture generator"]
fn generate_mainnet_state_patch() {
    let state_patch = serde_json::from_str::<Vec<StateEntryJson>>(include_str!(
        "./migration/mainnet_proxy_oracle_ixlmustry_ixlmusdc.json"
    ))
    .unwrap()
    .into_iter()
    .map(|entry| {
        (
            BASE64_STANDARD.decode(entry.key).unwrap(),
            BASE64_STANDARD.decode(entry.value).unwrap(),
        )
    })
    .collect::<fixture::StatePatch>();

    fs::write(
        fixture::mainnet_patch_path(),
        near_sdk::borsh::to_vec(&state_patch).unwrap(),
    )
    .unwrap();
}
