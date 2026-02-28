use near_jsonrpc_client::JsonRpcClient;
use near_workspaces::{network::Sandbox, Worker};
use templar_common::oracle::{
    price_transformer::{self, PriceTransformer, ProxyPriceTransformer},
    proxy::{Proxy, ProxyEntry},
    pyth::PriceIdentifier,
    OraclePriceId,
};
use templar_relayer::client::near::Near;
use test_utils::{
    accounts,
    controller::{lst_oracle::LstOracleController, proxy_oracle::ProxyOracleController},
    worker, ContractController, FtController, MockOracleController, DEFAULT_BORROW_PRICE_ID,
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
        lst,
        price_oracle,
        borrow_asset,
        proxy_oracle
    );

    let price_oracle_id = price_oracle.id().clone();

    let lst = LstOracleController::deploy(lst, price_oracle_id.clone());
    let proxy_oracle = ProxyOracleController::deploy(
        proxy_oracle,
        price_oracle_id.clone(),
        price_oracle_id.clone(),
    );
    let price_oracle = MockOracleController::deploy(price_oracle);
    let borrow_asset = FtController::deploy(borrow_asset, "Borrow Asset", "BAT");

    let (lst, price_oracle, borrow_asset, proxy_oracle) =
        tokio::join!(lst, price_oracle, borrow_asset, proxy_oracle);

    let near = Near::new(
        JsonRpcClient::connect(worker.rpc_addr()),
        relayer_user.id().clone(),
        vec![],
    );

    let resolved_normal = near
        .resolve_price_identifier(price_oracle_id.clone(), DEFAULT_BORROW_PRICE_ID)
        .await
        .unwrap();

    assert_eq!(
        resolved_normal,
        (
            price_oracle.id().clone(),
            OraclePriceId::from(DEFAULT_BORROW_PRICE_ID),
        ),
    );

    let resolved_passthrough = near
        .resolve_price_identifier(lst.id().clone(), DEFAULT_BORROW_PRICE_ID)
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
        .resolve_price_identifier(lst.id().clone(), proxy_id)
        .await
        .unwrap();

    assert_eq!(
        resolved_proxy,
        (
            price_oracle.id().to_owned(),
            OraclePriceId::from(DEFAULT_BORROW_PRICE_ID),
        ),
    );

    // Test proxy contract too
    let proxy_borrow = Proxy(vec![ProxyEntry::Pyth(DEFAULT_BORROW_PRICE_ID)]);

    let id = proxy_oracle
        .add_proxy(proxy_oracle.account(), proxy_borrow.clone())
        .await;

    assert_eq!(id, proxy_borrow.id().unwrap());

    let transform_borrow = Proxy(vec![ProxyEntry::Transformer(ProxyPriceTransformer::lst(
        OraclePriceId::Pyth(DEFAULT_BORROW_PRICE_ID),
        24,
        price_transformer::Call::new_simple(borrow_asset.id(), "redemption_rate"),
    ))]);

    let id = proxy_oracle
        .add_proxy(proxy_oracle.account(), transform_borrow.clone())
        .await;

    assert_eq!(id, transform_borrow.id().unwrap());

    // Passthrough
    let (oid, pid) = near
        .resolve_price_identifier(proxy_oracle.id().clone(), DEFAULT_BORROW_PRICE_ID)
        .await
        .unwrap();

    assert_eq!(&oid, price_oracle.id());
    assert_eq!(pid, OraclePriceId::from(DEFAULT_BORROW_PRICE_ID));

    // Direct Pyth proxy
    let (oid, pid) = near
        .resolve_price_identifier(proxy_oracle.id().clone(), proxy_borrow.id().unwrap())
        .await
        .unwrap();

    assert_eq!(&oid, price_oracle.id());
    assert_eq!(pid, OraclePriceId::from(DEFAULT_BORROW_PRICE_ID));

    let (oid, pid) = near
        .resolve_price_identifier(proxy_oracle.id().clone(), transform_borrow.id().unwrap())
        .await
        .unwrap();

    assert_eq!(&oid, price_oracle.id());
    assert_eq!(pid, OraclePriceId::from(DEFAULT_BORROW_PRICE_ID));
}
