#![allow(dead_code)]

use std::{collections::HashMap, path::PathBuf};

use near_sdk::serde_json::json;
use near_sdk::{borsh, json_types::Base64VecU8, NearToken};
use near_workspaces::{network::Sandbox, Contract, Worker};
use templar_common::{oracle::pyth::PriceIdentifier, time::Nanoseconds};
use templar_proxy_oracle_kernel::state::legacy::v0;
use templar_proxy_oracle_kernel::{price_transformer::Call, request::OracleRequest};
use test_utils::{workspace_root, ContractController, MockOracleController, ProxyOracleController};

pub type StatePatch = HashMap<Vec<u8>, Vec<u8>>;

pub const BTC_PRICE_ID: PriceIdentifier = PriceIdentifier([0x41; 32]);
pub const ETH_PRICE_ID: PriceIdentifier = PriceIdentifier([0x42; 32]);
pub const STNEAR_PRICE_ID: PriceIdentifier = PriceIdentifier([0x43; 32]);
pub const PENDING_PRICE_ID: PriceIdentifier = PriceIdentifier([0x44; 32]);

#[derive(Clone, Debug)]
pub struct FixtureAccounts {
    pub pyth: near_sdk::AccountId,
    pub pyth2: near_sdk::AccountId,
    pub redstone: near_sdk::AccountId,
}

pub fn patch_path() -> PathBuf {
    workspace_root()
        .join("contract/proxy-oracle/near/contract/tests/migration/v0_state_patch.borsh")
}

pub fn mainnet_patch_path() -> PathBuf {
    workspace_root().join(
        "contract/proxy-oracle/near/contract/tests/migration/mainnet_proxy_oracle_ixlmustry_ixlmusdc.borsh",
    )
}

pub fn load_state_patch() -> StatePatch {
    borsh::from_slice(include_bytes!("../migration/v0_state_patch.borsh")).unwrap()
}

pub fn load_mainnet_state_patch() -> StatePatch {
    borsh::from_slice(include_bytes!(
        "../migration/mainnet_proxy_oracle_ixlmustry_ixlmusdc.borsh"
    ))
    .unwrap()
}

pub async fn deploy_patched_with_state_patch(
    worker: &Worker<Sandbox>,
    state_patch: StatePatch,
) -> ProxyOracleController {
    let contract = worker
        .dev_deploy(ProxyOracleController::wasm_v0())
        .await
        .unwrap();

    for (key, value) in state_patch {
        worker
            .patch_state(contract.id(), &key, &value)
            .await
            .unwrap();
    }

    let contract = contract
        .as_account()
        .deploy(ProxyOracleController::wasm().await)
        .await
        .unwrap()
        .unwrap();

    ProxyOracleController { contract }
}

pub async fn deploy_patched(worker: &Worker<Sandbox>) -> ProxyOracleController {
    deploy_patched_with_state_patch(worker, load_state_patch()).await
}

pub async fn gov_create(contract: &Contract, id: u32, operation: &v0::Operation) {
    contract
        .as_account()
        .call(contract.id(), "gov_create")
        .args_json(json!({ "id": id, "operation": operation }))
        .deposit(NearToken::from_yoctonear(1))
        .max_gas()
        .transact()
        .await
        .unwrap()
        .unwrap();
}

pub async fn gov_execute(contract: &Contract, id: u32) {
    contract
        .as_account()
        .call(contract.id(), "gov_execute")
        .args_json(json!({ "id": id }))
        .deposit(NearToken::from_yoctonear(1))
        .max_gas()
        .transact()
        .await
        .unwrap()
        .unwrap();
}

pub fn executed_btc_proxy(accounts: &FixtureAccounts) -> v0::Proxy {
    v0::Proxy {
        aggregator: v0::Aggregator {
            method: v0::AggregationMethod::MedianLow,
            filter: v0::Filter {
                max_age: Some(Nanoseconds::from_secs(60)),
                max_clock_drift: Some(Nanoseconds::from_secs(10)),
                min_sources: Some(2),
            },
        },
        entries: vec![
            v0::Entry::new(OracleRequest::pyth(accounts.pyth.clone(), BTC_PRICE_ID), 3),
            v0::Entry::new(OracleRequest::redstone(accounts.redstone.clone(), "BTC"), 1),
        ],
    }
}

pub fn executed_eth_proxy(accounts: &FixtureAccounts) -> v0::Proxy {
    v0::Proxy {
        aggregator: v0::Aggregator {
            method: v0::AggregationMethod::Priority,
            filter: v0::Filter {
                max_age: Some(Nanoseconds::from_secs(70)),
                max_clock_drift: Some(Nanoseconds::from_secs(20)),
                min_sources: Some(1),
            },
        },
        entries: vec![
            v0::Entry::new(OracleRequest::redstone(accounts.redstone.clone(), "ETH"), 7),
            v0::Entry::new(OracleRequest::pyth(accounts.pyth.clone(), ETH_PRICE_ID), 7),
            v0::Entry::new(OracleRequest::pyth(accounts.pyth2.clone(), ETH_PRICE_ID), 3),
        ],
    }
}

pub fn executed_stnear_proxy(accounts: &FixtureAccounts) -> v0::Proxy {
    v0::Proxy {
        aggregator: v0::Aggregator {
            method: v0::AggregationMethod::MedianLow,
            filter: v0::Filter {
                max_age: Some(Nanoseconds::from_secs(120)),
                max_clock_drift: Some(Nanoseconds::from_secs(15)),
                min_sources: Some(1),
            },
        },
        entries: vec![
            v0::Entry::new(
                v0::ProxyPriceTransformer::lst(
                    OracleRequest::pyth(accounts.pyth.clone(), STNEAR_PRICE_ID),
                    24,
                    Call {
                        account_id: "wrap.near".parse().unwrap(),
                        method_name: "redemption_rate".to_string(),
                        args: Base64VecU8(
                            near_sdk::serde_json::to_vec(&near_sdk::serde_json::Value::Null)
                                .unwrap(),
                        ),
                        gas: near_sdk::Gas::from_tgas(3).as_gas().into(),
                    },
                ),
                2,
            ),
            v0::Entry::new(
                OracleRequest::redstone(accounts.redstone.clone(), "stNEAR"),
                1,
            ),
        ],
    }
}

pub fn pending_proxy(accounts: &FixtureAccounts) -> v0::Proxy {
    v0::Proxy {
        aggregator: v0::Aggregator {
            method: v0::AggregationMethod::Priority,
            filter: v0::Filter {
                max_age: Some(Nanoseconds::from_secs(30)),
                max_clock_drift: Some(Nanoseconds::from_secs(5)),
                min_sources: Some(1),
            },
        },
        entries: vec![
            v0::Entry::new(
                OracleRequest::pyth(accounts.pyth2.clone(), PENDING_PRICE_ID),
                11,
            ),
            v0::Entry::new(
                OracleRequest::redstone(accounts.redstone.clone(), "PENDING"),
                9,
            ),
            v0::Entry::new(
                OracleRequest::pyth(accounts.pyth.clone(), PENDING_PRICE_ID),
                9,
            ),
        ],
    }
}

async fn create_named_account(worker: &Worker<Sandbox>, name: &str) -> near_workspaces::Account {
    worker
        .root_account()
        .unwrap()
        .create_subaccount(name)
        .initial_balance(NearToken::from_near(10))
        .transact()
        .await
        .unwrap()
        .unwrap()
}

pub async fn create_fixture_accounts(worker: &Worker<Sandbox>) -> FixtureAccounts {
    let pyth = MockOracleController::deploy(create_named_account(worker, "pyth").await).await;
    let pyth2 = MockOracleController::deploy(create_named_account(worker, "pyth2").await).await;
    let redstone =
        MockOracleController::deploy(create_named_account(worker, "redstone").await).await;

    FixtureAccounts {
        pyth: pyth.id().clone(),
        pyth2: pyth2.id().clone(),
        redstone: redstone.id().clone(),
    }
}
