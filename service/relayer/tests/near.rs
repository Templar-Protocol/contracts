use near_jsonrpc_client::JsonRpcClient;
use near_workspaces::{network::Sandbox, Worker};
use templar_common::oracle::{
    price_transformer::{self, PriceTransformer, ProxyPriceTransformer},
    proxy::{Proxy, Source},
    pyth::PriceIdentifier,
    OracleRequest,
};
use templar_relayer::client::near::{Near, ResolvePriceIdentifierError};
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
    let proxy_oracle = ProxyOracleController::deploy(proxy_oracle);
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
        OracleRequest::pyth(price_oracle.id().clone(), DEFAULT_BORROW_PRICE_ID),
    );

    let resolved_passthrough = near
        .resolve_price_identifier(lst.id().clone(), DEFAULT_BORROW_PRICE_ID)
        .await
        .unwrap();

    assert_eq!(
        resolved_passthrough,
        OracleRequest::pyth(price_oracle.id().to_owned(), DEFAULT_BORROW_PRICE_ID),
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
        OracleRequest::pyth(price_oracle.id().to_owned(), DEFAULT_BORROW_PRICE_ID),
    );

    // Test proxy contract too
    let proxy_borrow =
        Proxy::median([
            OracleRequest::pyth(price_oracle.id().to_owned(), DEFAULT_BORROW_PRICE_ID).into(),
        ]);
    let proxy_borrow_id = PriceIdentifier([0x01_u8; 32]);

    proxy_oracle
        .set_proxy(proxy_oracle.account(), proxy_borrow_id, Some(proxy_borrow))
        .await;

    let transform_borrow = Proxy::median([Source::Transformer(ProxyPriceTransformer::lst(
        OracleRequest::pyth(price_oracle.id().to_owned(), DEFAULT_BORROW_PRICE_ID),
        24,
        price_transformer::Call::new_simple(borrow_asset.id(), "redemption_rate"),
    ))]);
    let transform_borrow_id = PriceIdentifier([0x02_u8; 32]);

    proxy_oracle
        .set_proxy(
            proxy_oracle.account(),
            transform_borrow_id,
            Some(transform_borrow.clone()),
        )
        .await;

    // Passthrough
    let request = near
        .resolve_price_identifier(proxy_oracle.id().clone(), DEFAULT_BORROW_PRICE_ID)
        .await;

    #[allow(clippy::match_wildcard_for_single_variants)]
    match request.unwrap_err() {
        ResolvePriceIdentifierError::NotFound {
            oracle_id,
            price_identifier,
        } => {
            assert_eq!(&oracle_id, proxy_oracle.id());
            assert_eq!(price_identifier, DEFAULT_BORROW_PRICE_ID);
        }
        err => {
            panic!("Expected NotFound error, got {err:?}");
        }
    }

    // Direct Pyth proxy
    let request = near
        .resolve_price_identifier(proxy_oracle.id().clone(), proxy_borrow_id)
        .await
        .unwrap();

    assert_eq!(
        request,
        OracleRequest::pyth(price_oracle.id().clone(), DEFAULT_BORROW_PRICE_ID)
    );

    // Transformed Pyth price
    let request = near
        .resolve_price_identifier(proxy_oracle.id().clone(), transform_borrow_id)
        .await
        .unwrap();

    assert_eq!(
        request,
        OracleRequest::pyth(price_oracle.id().clone(), DEFAULT_BORROW_PRICE_ID)
    );
}
