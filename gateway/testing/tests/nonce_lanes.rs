//! Concurrent writes on a single access key must not race its nonce (ENG-530).
//!
//! Node-backed: run with `just test-sandbox -p templar-gateway-testing`.

use anyhow::Result;
use rstest::rstest;
use templar_gateway_testing::{harness, SandboxHarness};

/// Every harness account signs with one access key, so these writes all contend
/// for the same nonce lane. Before per-key leasing, `near-api` allocated correct
/// monotonic nonces but the signed transactions were broadcast concurrently and
/// could reach the network out of order — NEAR rejects any nonce that is not
/// above the key's current one, so a fraction failed with `InvalidTransaction`
/// and were left submitted for reconciliation.
///
/// The harness builds a fresh `Client` per write, which is itself part of the
/// guarantee: the lanes live in the shared `PooledSigner`, not in the client.
#[rstest]
#[tokio::test]
async fn concurrent_writes_on_one_access_key_all_succeed(
    #[future(awt)] harness: SandboxHarness,
) -> Result<()> {
    const WRITERS: u128 = 8;

    let user = harness.create_user("nonce-lane").await?;
    harness
        .storage_deposit_min(&user, &harness.ft_contract_id)
        .await?;

    // Distinct amounts keep the operations distinct, so idempotency cannot
    // collapse them into a single write.
    let mints = futures::future::join_all(
        (1..=WRITERS).map(|amount| harness.mint(&user, &harness.ft_contract_id, amount)),
    )
    .await;

    // `mint` already fails unless the operation reached `Succeeded` with no
    // failed receipts.
    for mint in mints {
        mint?;
    }

    Ok(())
}
