use super::*;
use soroban_sdk::testutils::Address as _;

#[test]
fn test_settlement_receipt_new() {
    let receipt = SettlementReceipt::new(1, 2, 1000);
    assert_eq!(receipt.op_id, 1);
    assert_eq!(receipt.attempt_id, 2);
    assert_eq!(receipt.new_external_assets, 1000);
}

#[test]
fn test_market_ref_new() {
    let asset = AssetId::from([7u8; 32]);
    let market_ref: MarketRef = (42, asset.clone()).into();
    assert_eq!(market_ref.market_id, 42);
    assert_eq!(market_ref.asset_id, asset);
}

#[test]
fn test_test_market_adapter_success() {
    let adapter = TestMarketAdapter::new(1000);
    let env = Env::default();
    let asset = Address::generate(&env);

    assert!(adapter.supply(&env, &asset, 100).is_ok());
    assert!(adapter.withdraw(&env, &asset, 50).is_ok());
    assert_eq!(adapter.total_assets(&env, &asset).unwrap(), 1000);
}

#[test]
fn test_test_market_adapter_failure() {
    let adapter = TestMarketAdapter::failing();
    let env = Env::default();
    let asset = Address::generate(&env);

    assert!(adapter.supply(&env, &asset, 100).is_err());
    assert!(adapter.withdraw(&env, &asset, 50).is_err());
    assert!(adapter.total_assets(&env, &asset).is_err());
}

#[test]
fn test_cross_chain_adapter_submit_intent() {
    let adapter = TestCrossChainAdapter::new();
    let env = Env::default();
    let plan = Bytes::new(&env);

    let attempt_id = adapter.submit_intent(&env, plan).unwrap();
    assert_eq!(attempt_id, 1);
}

#[test]
fn test_cross_chain_adapter_settle() {
    let receipt = SettlementReceipt::new(10, 20, 5000);
    let adapter = TestCrossChainAdapter::new().with_settlement(receipt.clone());
    let env = Env::default();

    let result = adapter.settle(&env, 10, 20).unwrap();
    assert_eq!(result, receipt);
}

#[test]
fn test_cross_chain_adapter_total_assets() {
    let mut adapter = TestCrossChainAdapter::new();
    adapter.mock_total_assets = 2500;
    let env = Env::default();
    let asset = Address::generate(&env);

    assert_eq!(adapter.total_assets(&env, &asset).unwrap(), 2500);
}
