use near_account_id::AccountId;
use serde_json::json;
use templar_common::asset::{BorrowAssetAmount, CollateralAssetAmount};
use templar_gateway_methods_spec::{
    chain::GetBlock,
    market::*,
    proxy_oracle::Create as ProxyOracleCreate,
    registry::{Deploy, DeployTarget, ListDeploymentsByKind},
    universal_account::Create as UniversalAccountCreate,
};
use templar_gateway_types::{common::Pagination, contract::ContractKind, Base64Bytes, NearToken};
use templar_primitives::SU128;
use templar_universal_account::{transaction::Transaction, KeyId};

type UniversalAccountCreateConstructor =
    fn(DeployTarget, KeyId, SU128, Option<Box<[Transaction]>>) -> UniversalAccountCreate;

fn assert_universal_account_create_constructor(_: UniversalAccountCreateConstructor) {}

fn target() -> DeployTarget {
    DeployTarget {
        registry_id: near_account_id::AccountIdRef::new_or_panic("registry.near").to_owned(),
        name: "market".to_owned(),
        version_key: "v1.0.0".to_owned(),
        skip_abi_check: false,
        full_access_keys: None,
        deposit: NearToken::from_near(1),
    }
}

#[test]
fn liquidate_new_requires_collateral_amount_argument() {
    let request = Liquidate::new(
        "market.near".parse::<AccountId>().unwrap(),
        "alice.near".parse::<AccountId>().unwrap(),
        BorrowAssetAmount::new(100),
        Some(CollateralAssetAmount::new(25)),
    );

    assert_eq!(
        request,
        Liquidate {
            market_id: "market.near".parse().unwrap(),
            account_id: "alice.near".parse().unwrap(),
            liquidation_amount: BorrowAssetAmount::new(100),
            collateral_amount: Some(CollateralAssetAmount::new(25)),
        },
    );
    assert_eq!(
        serde_json::to_value(&request).unwrap(),
        json!({
            "market_id": "market.near",
            "account_id": "alice.near",
            "liquidation_amount": "100",
            "collateral_amount": "25",
        }),
    );
}

#[test]
fn liquidate_new_still_allows_explicit_none_for_backward_compatibility() {
    let request = Liquidate::new(
        "market.near".parse::<AccountId>().unwrap(),
        "alice.near".parse::<AccountId>().unwrap(),
        BorrowAssetAmount::new(100),
        None,
    );

    assert_eq!(
        request,
        Liquidate {
            market_id: "market.near".parse().unwrap(),
            account_id: "alice.near".parse().unwrap(),
            liquidation_amount: BorrowAssetAmount::new(100),
            collateral_amount: None,
        },
    );
    assert_eq!(
        serde_json::to_value(&request).unwrap(),
        json!({
            "market_id": "market.near",
            "account_id": "alice.near",
            "liquidation_amount": "100",
            "collateral_amount": null,
        }),
    );
}

#[test]
fn list_borrow_positions_new_defaults_args_to_pagination_default() {
    let request = ListBorrowPositions::new("market.near".parse().unwrap());

    assert_eq!(
        request,
        ListBorrowPositions {
            market_id: "market.near".parse().unwrap(),
            args: Pagination::default(),
        },
    );
    assert_eq!(
        serde_json::to_value(&request).unwrap(),
        json!({
            "market_id": "market.near",
            "offset": null,
            "count": null,
        }),
    );
}

#[test]
fn list_borrow_positions_with_args_preserves_flattened_json() {
    let request = ListBorrowPositions::new("market.near".parse().unwrap()).with_args(Pagination {
        offset: Some(1),
        limit: Some(50),
    });

    assert_eq!(
        request,
        ListBorrowPositions {
            market_id: "market.near".parse().unwrap(),
            args: Pagination {
                offset: Some(1),
                limit: Some(50),
            },
        },
    );
    assert_eq!(
        serde_json::to_value(&request).unwrap(),
        json!({
            "market_id": "market.near",
            "offset": 1,
            "count": 50,
        }),
    );
}

/// The target's fields stay at the top level of the wire JSON: flattening it is a
/// deduplication of the Rust declaration, not a params change.
#[test]
fn deploy_flattens_its_target_into_the_same_flat_json() {
    let request = Deploy::new(target(), Base64Bytes(vec![1, 2, 3]));

    assert_eq!(
        serde_json::to_value(&request).unwrap(),
        json!({
            "registry_id": "registry.near",
            "name": "market",
            "version_key": "v1.0.0",
            "init_args": "AQID",
            "deposit": NearToken::from_near(1).as_yoctonear().to_string(),
        }),
    );
    assert_eq!(
        serde_json::from_value::<Deploy>(serde_json::to_value(&request).unwrap()).unwrap(),
        request,
    );
}

/// A method's own init fields sit beside the target's on the wire, so the flat body
/// one method accepts is the flat body every other one accepts.
#[test]
fn a_create_reads_its_init_fields_beside_the_flat_target() {
    let mut body = serde_json::to_value(target()).unwrap();
    body["owner_id"] = json!("gov.near");

    let oracle = serde_json::from_value::<ProxyOracleCreate>(body).unwrap();
    assert_eq!(oracle.target, target());
    assert_eq!(oracle.owner_id, Some("gov.near".parse().unwrap()));
}

/// `ua.create` now names its sub-account with the shared `name` rather than its own
/// `account_name`, so its constructor takes the target in place of both.
#[test]
fn universal_account_create_new_accepts_four_required_arguments() {
    assert_universal_account_create_constructor(UniversalAccountCreate::new);
}

#[test]
fn list_deployments_by_kind_new_keeps_required_fields_after_defaulted_args() {
    let request = ListDeploymentsByKind::new("registry.near".parse().unwrap(), ContractKind::Vault);

    assert_eq!(
        request,
        ListDeploymentsByKind {
            registry_id: "registry.near".parse().unwrap(),
            args: Pagination::default(),
            kind: ContractKind::Vault,
        },
    );
    assert_eq!(
        serde_json::to_value(&request).unwrap(),
        json!({
            "registry_id": "registry.near",
            "offset": null,
            "count": null,
            "kind": "vault",
        }),
    );
}

#[test]
fn get_block_new_defaults_block_hash_to_none() {
    let request = GetBlock::new();

    assert_eq!(request, GetBlock { block_hash: None });
    assert_eq!(serde_json::to_value(&request).unwrap(), json!({}));
}
