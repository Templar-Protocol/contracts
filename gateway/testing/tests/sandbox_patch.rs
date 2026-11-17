//! Single-submission patch creation and exact metadata resets on the pinned node.
//!
//! Run with `just test-sandbox -p templar-gateway-testing --test sandbox_patch`.

use anyhow::Result;
use near_api::{types::AccountId, NetworkConfig};
use near_primitives::{
    account::{AccessKey, Account as ChainAccount, AccountContract},
    hash::hash,
    state_record::StateRecord,
};
use near_sandbox::Sandbox;
use near_token::NearToken;
use templar_gateway_core::{FinalityPolicy, NearClient};
use templar_gateway_testing::{sandbox_config, test_secret_key};
use templar_sandbox::{patch_records, wait_for_account_final, wait_until_final};

#[tokio::test]
async fn single_submission_patches_finalize_creation_and_exact_metadata() -> Result<()> {
    // An owned node also exercises the first patch after startup, before any transactions.
    let sandbox = Sandbox::start_sandbox_with_config(sandbox_config()).await?;
    let network = NetworkConfig::from_rpc_url("sandbox", sandbox.rpc_addr.parse()?);
    let near = NearClient::with_finality_policy(network.clone(), FinalityPolicy::Final);
    let public_key: near_crypto::PublicKey = test_secret_key()?.public_key().to_string().parse()?;
    let key = vec![0, 255, 1];
    let value = vec![255, 0, 2, 3];
    let code = b"\0asm\x01\0\0\0".to_vec();
    let code_hash = hash(&code);
    let storage_usage = 182 + 40 + key.len() as u64 + value.len() as u64 + code.len() as u64;

    for index in 0..16 {
        let account_id: AccountId = format!("patch-{index}.test.near").parse()?;
        patch_records(
            &network,
            vec![
                StateRecord::Account {
                    account_id: account_id.clone(),
                    account: ChainAccount::new(
                        NearToken::from_near(100_000_000),
                        NearToken::from_yoctonear(0),
                        AccountContract::Local(code_hash),
                        storage_usage,
                    ),
                },
                StateRecord::Data {
                    account_id: account_id.clone(),
                    data_key: key.clone().into(),
                    value: value.clone().into(),
                },
                StateRecord::Contract {
                    account_id: account_id.clone(),
                    code: code.clone(),
                },
                StateRecord::AccessKey {
                    account_id: account_id.clone(),
                    public_key: public_key.clone(),
                    access_key: AccessKey::full_access(),
                },
            ],
        )
        .await?;
        wait_until_final(&network, &account_id, &public_key).await?;

        // The key already exists: only the exact account view can fence this later patch.
        let restored = ChainAccount::new(
            NearToken::from_near(100 + index),
            NearToken::from_yoctonear(0),
            AccountContract::Local(code_hash),
            storage_usage,
        );
        let expected = (&restored).into();
        patch_records(
            &network,
            vec![StateRecord::Account {
                account_id: account_id.clone(),
                account: restored,
            }],
        )
        .await?;
        wait_for_account_final(&network, &account_id, &expected).await?;
        let entries = near
            .contract(account_id.clone())
            .state_with_prefix(Vec::new())
            .await?;
        assert_eq!(entries.len(), 1);
        let (actual_key, actual_value) = &entries[0];
        assert_eq!(actual_key.0, key);
        assert_eq!(actual_value.0, value);
        assert_eq!(near.contract(account_id).code().await?.0, code);
    }

    Ok(())
}
