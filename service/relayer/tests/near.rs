use near_jsonrpc_client::JsonRpcClient;

use templar_common::oracle::{
    price_transformer::{self, PriceTransformer},
    pyth::PriceIdentifier,
};
use templar_relayer::client::near::Near;
use test_utils::*;

#[rstest::rstest]
#[tokio::test]
async fn transformer_resolution(#[future(awt)] worker: Sandbox) {
    setup_test!(worker extract(c) accounts(relayer_user, lst_account));

    let lst = LstOracleController::deploy(lst_account, c.price_oracle.account.id()).await;

    let near = Near::new(
        JsonRpcClient::connect(&worker.rpc_addr),
        relayer_user.id().clone(),
        vec![],
    );

    let resolved_normal = near
        .try_resolve_price_identifier(c.price_oracle.account.id().clone(), DEFAULT_BORROW_PRICE_ID)
        .await
        .unwrap();

    assert_eq!(resolved_normal, DEFAULT_BORROW_PRICE_ID);

    let resolved_passthrough = near
        .try_resolve_price_identifier(lst.account.id().clone(), DEFAULT_BORROW_PRICE_ID)
        .await
        .unwrap();

    assert_eq!(resolved_passthrough, DEFAULT_BORROW_PRICE_ID);

    let proxy_id = PriceIdentifier([0xa6; 32]);

    lst.create_transformer(
        lst.account(),
        proxy_id,
        PriceTransformer::lst(
            DEFAULT_BORROW_PRICE_ID,
            24,
            price_transformer::Call::new_simple(c.borrow_asset.account().id(), "redemption_rate"),
        ),
    )
    .await;

    let resolved_proxy = near
        .try_resolve_price_identifier(lst.account.id().clone(), proxy_id)
        .await
        .unwrap();

    assert_eq!(resolved_proxy, DEFAULT_BORROW_PRICE_ID);
}
