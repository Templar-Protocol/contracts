use anyhow::Result;
use near_api::Contract;
use near_token::NearToken;
use serde_json::json;
use templar_gateway_methods_spec::token::{GetMetadata, TokenReference};
use templar_gateway_testing::{wasm, SandboxHarness, TEST_FINALITY_POLICY};

#[tokio::test]
async fn opaque_metadata_requires_exact_mt_identity_not_an_embedded_ft_prefix() -> Result<()> {
    let harness = SandboxHarness::start().await?;
    let ft = harness
        .deploy_ft("metadata-ft", "Fixture FT", "FFT")
        .await?;
    let client = harness.client()?;

    let opaque_id = "nep245:wrapper.near:chain:asset";
    let (opaque_mt, signer) = harness
        .create_account_via_tx("opaque-mt", NearToken::from_near(25))
        .await?;
    Contract::deploy(opaque_mt.clone())
        .use_code(wasm::mt().await.to_vec())
        .with_init_call("new", json!({"extra_token_ids": [opaque_id]}))?
        .with_signer(signer)
        .wait_until(TEST_FINALITY_POLICY.transaction_status())
        .send_to(&harness.network)
        .await?
        .assert_success();
    let accepted = client
        .read(GetMetadata {
            token: TokenReference::Mt {
                contract_id: opaque_mt.clone(),
                token_id: opaque_id.to_owned(),
            },
        })
        .await?;
    assert_eq!(accepted.decimals, 24);
    // Neither truncating an opaque ID nor an embedded NEP-141 prefix identifies
    // a token or permits a fallback to a different contract's metadata.
    for absent_id in [
        "nep245:wrapper.near:chain".to_owned(),
        format!("nep141:{ft}"),
    ] {
        assert!(client
            .read(GetMetadata {
                token: TokenReference::Mt {
                    contract_id: opaque_mt.clone(),
                    token_id: absent_id,
                },
            })
            .await
            .is_err());
    }
    Ok(())
}
