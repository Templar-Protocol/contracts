use rstest::rstest;
use test_utils::*;

#[rstest]
#[tokio::test]
async fn lst_oracle() {
    let worker = near_workspaces::sandbox().await.unwrap();
    accounts!(worker, lstoracle);

    setup_test_w!(worker extract(c) accounts() config(|_| { }));
}
