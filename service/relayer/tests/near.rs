use near_jsonrpc_client::JsonRpcClient;
use near_workspaces::{network::Sandbox, Worker};
use templar_common::oracle::{
    price_transformer::{self, PriceTransformer},
    pyth::PriceIdentifier,
    OraclePriceId,
};
use templar_relayer::client::near::Near;
use test_utils::{
    accounts, controller::lst_oracle::LstOracleController, worker, ContractController,
    FtController, MockOracleController, DEFAULT_BORROW_PRICE_ID,
};

#[rstest::rstest]
#[tokio::test]
async fn transformer_resolution(#[future(awt)] worker: Worker<Sandbox>) {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::new(
            "templar_relayer=debug,info",
        ))
        .try_init();

    accounts!(
        worker,
        relayer_user,
        lst_account,
        price_oracle,
        borrow_asset
    );

    let lst = LstOracleController::deploy(lst_account, price_oracle.id().clone());
    let price_oracle = MockOracleController::deploy(price_oracle);
    let borrow_asset = FtController::deploy(borrow_asset, "Borrow Asset", "BAT");

    let (lst, price_oracle, borrow_asset) = tokio::join!(lst, price_oracle, borrow_asset);

    let near = Near::new(
        JsonRpcClient::connect(worker.rpc_addr()),
        relayer_user.id().clone(),
        vec![],
    );

    let resolved_normal = near
        .try_resolve_price_identifier(price_oracle.id().clone(), DEFAULT_BORROW_PRICE_ID)
        .await
        .unwrap();

    assert_eq!(
        resolved_normal,
        (
            price_oracle.id().to_owned(),
            OraclePriceId::from(DEFAULT_BORROW_PRICE_ID),
        ),
    );

    let resolved_passthrough = near
        .try_resolve_price_identifier(lst.contract.id().clone(), DEFAULT_BORROW_PRICE_ID)
        .await
        .unwrap();

    assert_eq!(
        resolved_passthrough,
        (
            price_oracle.id().to_owned(),
            OraclePriceId::from(DEFAULT_BORROW_PRICE_ID),
        ),
    );

    let proxy_id = PriceIdentifier([0xa6; 32]);

    lst.create_transformer(
        lst.contract.as_account(),
        proxy_id,
        PriceTransformer::lst(
            DEFAULT_BORROW_PRICE_ID,
            24,
            price_transformer::Call::new_simple(borrow_asset.id(), "redemption_rate"),
        ),
    )
    .await;

    let resolved_proxy = near
        .try_resolve_price_identifier(lst.contract.id().clone(), proxy_id)
        .await
        .unwrap();

    assert_eq!(
        resolved_proxy,
        (
            price_oracle.id().to_owned(),
            OraclePriceId::from(DEFAULT_BORROW_PRICE_ID),
        ),
    );

    // TODO: Test proxy contract too
}
