use near_account_id::AccountId;
use serde_json::json;
use templar_common::asset::{BorrowAssetAmount, CollateralAssetAmount};
use templar_gateway_methods_spec::{
    chain::GetBlock,
    market::*,
    registry::{Deploy, ListDeploymentsByKind},
};
use templar_gateway_types::{common::Pagination, contract::ContractKind, Base64Bytes, NearToken};

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

#[test]
fn deploy_new_defaults_full_access_keys_to_none() {
    let request = Deploy::new(
        "registry.near".parse().unwrap(),
        "market".to_owned(),
        "v1.0.0".to_owned(),
        Base64Bytes(vec![1, 2, 3]),
        NearToken::from_near(1),
    );

    assert_eq!(
        request,
        Deploy {
            registry_id: "registry.near".parse().unwrap(),
            name: "market".to_owned(),
            version_key: "v1.0.0".to_owned(),
            init_args: Base64Bytes(vec![1, 2, 3]),
            full_access_keys: None,
            deposit: NearToken::from_near(1),
        },
    );
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
