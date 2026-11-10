//! Freeze a causal raw-storage checkpoint before a potentially delayed DAO vote.
//! Replay the exact fetched oracle WASM and complete state using the same native
//! sandbox machinery as patch dry-run; never assume an oracle storage layout.

use anyhow::{ensure, Context, Result};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use serde::{ser::SerializeSeq as _, Serialize, Serializer as _};
use sha2::{Digest as _, Sha256};
use templar_gateway_methods_spec::{account, chain, tx};
use templar_gateway_types::{
    common::ContractArgs, ActionInput, Base64Bytes, ManagedAccountId, NearGas, NearToken,
    OperationStatus,
};
use templar_proxy_oracle_near_common::event::MAX_MANUAL_TRIP_METADATA_LEN;
use templar_proxy_oracle_near_governance_common::target;

use crate::{
    commands::proxy_oracle::PredictManualTrip,
    context::{print_json, CliContext},
    dispatch::{
        patch_state::{fetch_complete_state, snapshot_final, RawStateEntry},
        sandbox_replay::{
            build_local_client, reset_account_metadata, setup_account, setup_signer,
            stage_snapshot_code, start_sandbox,
        },
    },
};

#[derive(Debug, Serialize)]
struct StorageCheckpoint {
    storage_digest: String,
    storage_entries: usize,
}

#[derive(Serialize)]
struct Prediction {
    oracle_id: near_account_id::AccountId,
    block_hash: near_api::types::CryptoHash,
    effective_code_sha256: String,
    before: StorageCheckpoint,
    after: StorageCheckpoint,
}

pub(super) async fn predict(ctx: CliContext, args: PredictManualTrip) -> Result<()> {
    let oracle_id = args.target.resolve(&ctx).await?;
    let metadata = STANDARD
        .decode(&args.metadata_base64)
        .context("decode manual-trip metadata")?;
    ensure!(
        metadata.len() <= MAX_MANUAL_TRIP_METADATA_LEN,
        "manual-trip metadata is too long"
    );
    ensure!(
        oracle_id != args.actor,
        "oracle and governance actor must differ"
    );
    let call = target::admin_set_manual_trip(
        args.price_id,
        args.tripped,
        Some(metadata),
        Some(NearGas::from_tgas(100)),
    )?;
    let mut reporter = ctx.reporter(&[]);
    let limits = ctx.client.read(chain::GetProtocolLimits).await?;
    let snapshot = snapshot_final(
        &ctx,
        &oracle_id,
        &limits,
        "proxy_oracle.state_complete",
        &mut reporter,
    )
    .await?;
    let before = checkpoint(&snapshot.entries)?;
    let (sandbox, network) = start_sandbox().await?;
    let oracle_key = setup_account(&network, &oracle_id, None, &snapshot).await?;
    stage_snapshot_code(&network, &oracle_id, &snapshot, &oracle_key).await?;
    let staging_client = build_local_client(&network, &[(&oracle_id, &oracle_key)])?;
    let removal = staging_client
        .execute_as(
            ManagedAccountId(oracle_id.clone()),
            account::DeleteKey {
                public_key: oracle_key.public_key().into(),
            },
        )
        .await?;
    ensure!(
        removal.operation.status == OperationStatus::Succeeded,
        "sandbox staging-key removal failed: {:?}",
        removal.operation.status
    );
    // Retain the exact source identity, balance and storage footprint. Localizing
    // global code would impose collateral requirements absent from the source.
    reset_account_metadata(&network, &oracle_id, &snapshot).await?;
    let actor_key = setup_signer(&network, &args.actor).await?;
    let client = build_local_client(&network, &[(&args.actor, &actor_key)])?;
    let execution = client
        .execute_as(
            ManagedAccountId(args.actor),
            tx::Batch {
                receiver_id: oracle_id.clone(),
                actions: vec![ActionInput::FunctionCall {
                    method_name: call.method_name.into(),
                    args: ContractArgs::Raw(Base64Bytes(call.args.0)),
                    gas: call.gas,
                    deposit: NearToken::from_yoctonear(call.attached_deposit.0),
                }],
            },
        )
        .await?;
    ensure!(
        execution.operation.status == OperationStatus::Succeeded,
        "sandbox manual-trip prediction failed: {:?}",
        execution.operation.status
    );
    let block = client.read(chain::GetBlock::default()).await?;
    let predicted = fetch_complete_state(&network, &oracle_id, block.hash.into(), &limits).await?;
    let after = checkpoint(&predicted.entries)?;
    ensure!(
        before.storage_digest != after.storage_digest,
        "manual-trip prediction requires an actual state change"
    );
    reporter.digest();
    print_json(&Prediction {
        oracle_id,
        block_hash: snapshot.block_hash,
        effective_code_sha256: hex::encode(Sha256::digest(&snapshot.code)),
        before,
        after,
    })?;
    drop(sandbox);
    Ok(())
}

/// Complete-state fetching already sorts raw keys and rejects duplicates.
fn checkpoint(storage: &[RawStateEntry]) -> Result<StorageCheckpoint> {
    let mut hash = Sha256::new();
    hash.update(b"templar-asset-migration-v1\0");
    let mut serializer = serde_json::Serializer::new(&mut hash);
    let mut entries = serializer.serialize_seq(Some(storage.len()))?;
    for entry in storage {
        entries.serialize_element(&[STANDARD.encode(&entry.key), STANDARD.encode(&entry.value)])?;
    }
    entries.end()?;
    Ok(StorageCheckpoint {
        storage_digest: format!("sha256:{}", hex::encode(hash.finalize())),
        storage_entries: storage.len(),
    })
}
