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

pub const PATCH_PLAN_SCHEMA_VERSION: u32 = 3;

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
    pub target_code_hash: CryptoHash,
    pub state_digest: String,
    pub restore: RestoreCode,
    pub batch: tx::Batch,
    pub unguarded: bool,
    pub checks: Vec<Check>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dry_run: Option<DryRunStamp>,
}
impl PatchPlan {
    pub fn unstamped_digest(&self) -> anyhow::Result<String> {
        let mut plan = self.clone();
        plan.dry_run = None;
        Ok(super::plan::digest(&serde_json::to_vec(&plan)?))
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DryRunStamp {
    pub plan_digest: String,
    pub sandbox_chain_id: String,
    pub target_code_hash: CryptoHash,
    pub state_digest: String,
    pub checks: Vec<Check>,
}
