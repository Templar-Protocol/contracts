use super::*;

#[tokio::test]
async fn rpc_discover_returns_openrpc_document() -> Result<()> {
    let stack = TestStack::start().await?;

    let document = rpc_discover(&stack).await?;

    assert_eq!(document["openrpc"], serde_json::json!("1.4.2"));
    assert_eq!(
        document["info"]["title"],
        serde_json::json!("Blockchain Gateway RPC")
    );

    let methods = document["methods"]
        .as_array()
        .expect("methods should be an array");
    assert!(methods.iter().any(|method| method["name"] == "account.get"));
    assert!(methods
        .iter()
        .any(|method| method["name"] == "rpc.discover"));

    let account_get = methods
        .iter()
        .find(|method| method["name"] == "account.get")
        .expect("account.get should be documented");
    assert_eq!(
        account_get["summary"],
        serde_json::json!("Get chain state for a NEAR account.")
    );
    assert_eq!(
        account_get["tags"],
        serde_json::json!([{ "name": "account" }])
    );
    assert!(account_get["params"].is_array());

    stack.shutdown().await;
    Ok(())
}
