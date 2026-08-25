use near_account_id::AccountId;
use near_api::PublicKey;
use serde::{Deserialize, Serialize};
use templar_gateway_methods_spec::tx;

use super::{
    check::Check,
    patch::{PatchSpec, ResolvedPatch},
};

pub const PATCH_PLAN_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PatchPlan {
    pub schema: u32,
    pub tool_version: String,
    pub spec: PatchSpec,
    pub resolved: ResolvedPatch,
    pub signer_id: AccountId,
    pub public_key: PublicKey,
    pub patch_wasm_sha256: String,
    pub restore_code_hash: String,
    pub global_contract_hash: Option<String>,
    pub global_contract_account_id: Option<AccountId>,
    pub batch: tx::Batch,
    pub unguarded: bool,
    pub checks: Vec<Check>,
}
