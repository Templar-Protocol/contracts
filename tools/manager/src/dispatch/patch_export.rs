use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{ensure, Context, Result};
use base64::Engine as _;
use serde::Serialize;
use sha2::{Digest as _, Sha256};
use templar_gateway_methods_spec::chain;

use crate::{
    commands::patch::Export,
    context::{print_json, CliContext},
    dispatch::patch_state::fetch_complete_state,
    spec::{
        check::{gate, Check, Status},
        patch::{ByteExpr, Expectation, PatchCheck, PatchSpec, PatchStorageCheck},
    },
};

#[derive(Serialize)]
struct ExportReport {
    spec: PathBuf,
    blobs: PathBuf,
    entries: usize,
}

pub(super) async fn export(ctx: CliContext, args: Export) -> Result<()> {
    let mut reporter = ctx.reporter(&[]);
    let block = ctx.final_client()?.read(chain::GetBlock::default()).await?;
    let limits = ctx.client.read(chain::GetProtocolLimits).await?;
    let state = fetch_complete_state(
        ctx.network_config(),
        &args.account_id,
        block.hash.into(),
        &limits,
    )
    .await;
    let state = match state {
        Ok(state) => {
            reporter.record(Check::new(
                "patch.state_complete",
                Status::passed(format!(
                    "complete {} {} storage entries in {} request(s), accounting for {} bytes at {}",
                    if state.chunked { "after chunking" } else { "in one request" },
                    state.entries.len(),
                    state.request_count,
                    state.storage_usage,
                    state.block_hash
                )),
            ));
            state
        }
        Err(error) => {
            reporter.record(Check::new(
                "patch.state_complete",
                Status::failed(error.to_string()),
            ));
            reporter.digest();
            return Err(error);
        }
    };
    gate(
        reporter.checks(),
        args.account_id.as_str(),
        "no patch spec was written",
    )?;

    let blob_dir = blob_dir(&args.out)?;
    ensure!(
        !args.out.exists(),
        "refusing to overwrite {}",
        args.out.display()
    );
    ensure!(
        !blob_dir.exists(),
        "refusing to overwrite {}",
        blob_dir.display()
    );
    let spec_tmp = temporary_sibling(&args.out)?;
    let blobs_tmp = temporary_sibling(&blob_dir)?;
    ensure!(!spec_tmp.exists(), "temporary export path already exists");
    ensure!(!blobs_tmp.exists(), "temporary export path already exists");

    let result = write_export(
        &args.out,
        &blob_dir,
        &spec_tmp,
        &blobs_tmp,
        &args.account_id,
        &state,
    );
    if result.is_err() {
        let _ = fs::remove_file(&spec_tmp);
        let _ = fs::remove_dir_all(&blobs_tmp);
    }
    result?;
    reporter.digest();
    print_json(&ExportReport {
        spec: args.out,
        blobs: blob_dir,
        entries: state.entries.len(),
    })
}

fn blob_dir(spec: &Path) -> Result<PathBuf> {
    let stem = spec
        .file_stem()
        .context("patch export output must have a file name")?;
    Ok(spec.with_file_name(format!("{}.blobs", stem.to_string_lossy())))
}

fn temporary_sibling(path: &Path) -> Result<PathBuf> {
    let file_name = path
        .file_name()
        .context("patch export output must have a file name")?;
    Ok(path.with_file_name(format!(
        ".{}.tmp-{}",
        file_name.to_string_lossy(),
        std::process::id()
    )))
}

fn write_export(
    spec_path: &Path,
    blob_path: &Path,
    spec_tmp: &Path,
    blobs_tmp: &Path,
    account_id: &near_account_id::AccountId,
    state: &crate::dispatch::patch_state::StateSnapshot,
) -> Result<()> {
    fs::create_dir(blobs_tmp).with_context(|| format!("create {}", blobs_tmp.display()))?;
    let blob_name = blob_path
        .file_name()
        .context("patch export blob directory must have a file name")?
        .to_string_lossy();
    let checks = state
        .entries
        .iter()
        .map(|entry| {
            let digest = hex::encode(Sha256::digest(&entry.key));
            let file = format!("{blob_name}/{digest}.bin");
            fs::write(blobs_tmp.join(format!("{digest}.bin")), &entry.value)
                .with_context(|| format!("write exported value {file}"))?;
            Ok(PatchCheck::Storage(PatchStorageCheck {
                key: ByteExpr::Base64(base64::engine::general_purpose::STANDARD.encode(&entry.key)),
                expect: Expectation::Bytes(ByteExpr::File(PathBuf::from(file))),
            }))
        })
        .collect::<Result<Vec<_>>>()?;
    let spec = PatchSpec {
        schema: crate::spec::patch::PATCH_SCHEMA_VERSION,
        extends: Vec::new(),
        account_id: account_id.clone(),
        ops: Vec::new(),
        checks,
    };
    let rendered = toml::to_string_pretty(&spec).context("render exported patch spec")?;
    fs::write(spec_tmp, format!("{rendered}\n"))
        .with_context(|| format!("write {}", spec_tmp.display()))?;
    fs::rename(blobs_tmp, blob_path).with_context(|| format!("publish {}", blob_path.display()))?;
    if let Err(error) = fs::rename(spec_tmp, spec_path) {
        let _ = fs::remove_dir_all(blob_path);
        return Err(error).with_context(|| format!("publish {}", spec_path.display()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dispatch::patch_state::{RawStateEntry, StateSnapshot};
    use near_token::NearToken;

    #[test]
    fn export_writes_round_trippable_spec_and_blobs() {
        let root = std::env::temp_dir().join(format!(
            "templar-manager-export-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&root).unwrap();
        let spec_path = root.join("state.toml");
        let blob_path = root.join("state.blobs");
        let spec_tmp = temporary_sibling(&spec_path).unwrap();
        let blobs_tmp = temporary_sibling(&blob_path).unwrap();
        let account_id: near_account_id::AccountId = "target.near".parse().unwrap();
        let state = StateSnapshot {
            amount: NearToken::from_yoctonear(0),
            locked: NearToken::from_yoctonear(0),
            storage_usage: 0,
            contract: near_primitives::account::AccountContract::None,
            code: Vec::new(),
            access_keys: Vec::new(),
            entries: vec![
                RawStateEntry {
                    key: vec![1],
                    value: vec![3],
                },
                RawStateEntry {
                    key: vec![2],
                    value: vec![4, 5],
                },
            ],
            block_hash: near_api::types::CryptoHash([0; 32]),
            chunked: false,
            request_count: 1,
        };
        write_export(
            &spec_path,
            &blob_path,
            &spec_tmp,
            &blobs_tmp,
            &account_id,
            &state,
        )
        .unwrap();
        let loaded = crate::spec::patch::PatchSpec::load(&spec_path).unwrap();
        let resolved = loaded.resolve(&spec_path).unwrap();
        assert_eq!(loaded.schema, 3);
        assert_eq!(resolved.operations.len(), 2);
        assert_eq!(
            resolved.operations[0],
            crate::spec::patch::ResolvedOperation::Expect {
                key: templar_gateway_types::Base64Bytes(vec![1]),
                expected: crate::spec::patch::ResolvedExpectation::Bytes(
                    templar_gateway_types::Base64Bytes(vec![3])
                ),
            }
        );
        fs::remove_dir_all(root).unwrap();
    }
}
