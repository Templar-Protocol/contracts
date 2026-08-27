use std::path::PathBuf;

use near_account_id::AccountId;
use near_api::PublicKey;
use serde::{Deserialize, Serialize};
use templar_gateway_methods_spec::tx;
use templar_gateway_types::CryptoHash;

use super::{
    check::Check,
    patch::{PatchSpec, ResolvedPatch, Sha256Digest},
};

pub const PATCH_PLAN_SCHEMA_VERSION: u32 = 2;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
pub enum RestoreCode {
    Local { code_hash: CryptoHash },
    GlobalCodeHash { hash: CryptoHash },
    GlobalAccount { account_id: AccountId },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PatchPlan {
    pub schema: u32,
    pub tool_version: String,
    pub source_path: PathBuf,
    pub spec: PatchSpec,
    pub resolved: ResolvedPatch,
    pub signer_id: AccountId,
    pub public_key: PublicKey,
    pub patch_wasm_sha256: Sha256Digest,
    pub restore: RestoreCode,
    pub batch: tx::Batch,
    pub unguarded: bool,
    pub checks: Vec<Check>,
}
