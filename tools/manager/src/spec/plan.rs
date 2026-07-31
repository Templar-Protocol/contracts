//! The plan artifact: a serializable, hand-editable form of an
//! [`OperationPlan`].
//!
//! This is the escape hatch. A spec cannot express every market, and a check can
//! be wrong; when either happens, an operator needs to see the concrete
//! transactions, change one, and send the result. So `market plan` writes this
//! file, and `market apply` sends it.
//!
//! `gateway/core` is deliberately untouched. [`PlannedTransaction`] already
//! derives `Serialize`/`Deserialize`, so the only thing missing was a *legible*
//! encoding — which is a property of this tool, not of the gateway. The step
//! `label` lives here for the same reason: it exists to orient a human reading a
//! diff, and the executor has no use for it.
//!
//! ## Why JSON args are canonicalized
//!
//! `near_api` serializes `FunctionCallAction::args` as base64, which is correct
//! on the wire and useless in a file someone has to edit. So JSON args are
//! carried as JSON — but re-encoding through [`serde_json::Value`] sorts object
//! keys, while the gateway produced them in struct-declaration order. The bytes
//! therefore differ from the originals.
//!
//! Rather than pretend otherwise, conversion *into* this file canonicalizes.
//! The invariant that matters is not "the bytes are untouched" but **what
//! `apply` sends is exactly what `plan` showed** — so the canonical form is the
//! one that gets digested, displayed, and executed. Key order carries no meaning
//! to a contract decoding with `serde_json`, so nothing on chain can observe the
//! difference.

use anyhow::Context as _;
use near_account_id::AccountId;
use near_api::types::transaction::actions::{Action, FunctionCallAction};
use near_api::types::NearToken;
use serde::{Deserialize, Serialize};
use templar_gateway_core::{OperationPlan, PlannedTransaction};
use templar_gateway_types::{Base64Bytes, ManagedAccountId, NearGas};

use super::check::Check;

/// Bumped when this artifact's shape changes.
///
/// `apply` hard-refuses a mismatch. Every struct here is `deny_unknown_fields`,
/// so a plan written by a newer tool is rejected outright rather than applied
/// with fields this build cannot see — and this file authorizes spending real
/// NEAR.
pub const PLAN_SCHEMA_VERSION: u32 = 1;

/// A function call's arguments, in whichever form a human can actually edit.
///
/// Base64 is the wire encoding and `near_api`'s native serialization, but a plan
/// of opaque blobs cannot be edited, which is the whole point of the artifact.
/// So JSON args — nearly all of them — are carried as JSON, and base64 is
/// reserved for the genuinely opaque case (`registry add_version` takes borsh).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlanArgs {
    Json(serde_json::Value),
    Base64(Base64Bytes),
}

impl PlanArgs {
    /// Classify by probing the bytes, not by a list of method names — a list
    /// would silently drift from the methods it describes, and the failure mode
    /// is an unreadable plan rather than an error.
    ///
    /// Only a JSON *object* counts. Every NEAR contract takes its JSON args as
    /// one, so requiring it costs nothing and rules out a borsh payload that
    /// happens to parse as a bare JSON scalar (`5`, `null`, a quoted string).
    fn from_bytes(args: Vec<u8>) -> Self {
        match serde_json::from_slice::<serde_json::Value>(&args) {
            Ok(value) if value.is_object() => Self::Json(value),
            _ => Self::Base64(Base64Bytes(args)),
        }
    }

    fn into_bytes(self) -> anyhow::Result<Vec<u8>> {
        match self {
            Self::Json(value) => serde_json::to_vec(&value).context("encode JSON args"),
            Self::Base64(bytes) => Ok(bytes.0),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanFunctionCall {
    pub method_name: String,
    pub args: PlanArgs,
    /// Gas units. A plain integer rather than a `NearGas` string, so editing it
    /// needs no knowledge of that crate's parse format.
    pub gas: u64,
    pub deposit: NearToken,
}

/// One transaction.
///
/// Every step a deployment plans is one or more function calls; a plan carrying
/// any other action kind is refused at conversion rather than silently dropped,
/// so `function_calls` is the field rather than a general `actions` list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanStep {
    /// What this step is for, in words. Manager-only: the executor never reads
    /// it, and it exists so a diff of an edited plan is legible.
    pub label: String,
    pub signer_id: AccountId,
    pub receiver_id: AccountId,
    #[serde(default, skip_serializing_if = "is_false")]
    pub continue_on_failure: bool,
    pub function_calls: Vec<PlanFunctionCall>,
}

#[expect(
    clippy::trivially_copy_pass_by_ref,
    reason = "serde predicate signature"
)]
fn is_false(value: &bool) -> bool {
    !*value
}

impl PlanStep {
    fn from_planned(label: String, transaction: PlannedTransaction) -> anyhow::Result<Self> {
        let function_calls = transaction
            .actions
            .into_iter()
            .map(|action| match action {
                Action::FunctionCall(call) => Ok(PlanFunctionCall {
                    method_name: call.method_name,
                    args: PlanArgs::from_bytes(call.args),
                    gas: call.gas.as_gas(),
                    deposit: call.deposit,
                }),
                other => anyhow::bail!(
                    "a deployment plan carries only function calls, but `{label}` \
                     planned {other:?}. This is a bug in the plan builder, not \
                     something an edited plan can cause."
                ),
            })
            .collect::<anyhow::Result<Vec<_>>>()?;

        Ok(Self {
            label,
            signer_id: transaction.signer_account_id.0,
            receiver_id: transaction.receiver_id,
            continue_on_failure: transaction.continue_on_failure,
            function_calls,
        })
    }

    fn into_planned(self) -> anyhow::Result<PlannedTransaction> {
        let actions = self
            .function_calls
            .into_iter()
            .map(|call| {
                Ok(Action::FunctionCall(Box::new(FunctionCallAction {
                    method_name: call.method_name,
                    args: call.args.into_bytes()?,
                    gas: NearGas::from_gas(call.gas),
                    deposit: call.deposit,
                })))
            })
            .collect::<anyhow::Result<Vec<_>>>()?;

        Ok(PlannedTransaction {
            signer_account_id: ManagedAccountId::from(self.signer_id),
            receiver_id: self.receiver_id,
            actions,
            continue_on_failure: self.continue_on_failure,
        })
    }

    /// The digest recorded for this step at generation time.
    fn digest(&self) -> anyhow::Result<String> {
        digest(self)
    }
}

/// Values the spec implies rather than states, recorded so a reader does not
/// have to re-derive them — and so a reviewer can see what the tool concluded.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Derived {
    pub market_id: AccountId,
    pub oracle_id: AccountId,
    pub governance_id: AccountId,
    pub collateral_decimals: Option<u8>,
    pub borrow_decimals: Option<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanFile {
    pub schema: u32,
    pub tool_version: String,
    pub network: String,
    /// Digest of the spec this plan came from. Reported on apply so a plan can
    /// be traced back to its source, including after the spec has moved on.
    pub spec_digest: String,
    /// Per-step digests as generated.
    ///
    /// One per step rather than a single whole-file digest: with seven steps,
    /// "something changed" is not actionable, and naming the changed steps is
    /// what lets an operator confirm the edit they meant to make is the only one
    /// present.
    pub step_digests: Vec<String>,
    pub derived: Derived,
    pub checks: Vec<Check>,
    pub steps: Vec<PlanStep>,
}

/// What changed between a plan as generated and the file as it stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Drift {
    /// Indices of steps whose content differs from the recorded digest.
    pub changed: Vec<usize>,
    pub added: usize,
    pub removed: usize,
}

impl Drift {
    pub fn is_clean(&self) -> bool {
        self.changed.is_empty() && self.added == 0 && self.removed == 0
    }

    /// A plain sentence for the confirmation prompt.
    pub fn describe(&self) -> String {
        if self.is_clean() {
            return "plan is unmodified since generation".to_owned();
        }

        let mut parts = Vec::new();
        if !self.changed.is_empty() {
            parts.push(format!(
                "{} step(s) differ (#{})",
                self.changed.len(),
                self.changed
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(", #")
            ));
        }
        if self.added > 0 {
            parts.push(format!("{} step(s) added", self.added));
        }
        if self.removed > 0 {
            parts.push(format!("{} step(s) removed", self.removed));
        }
        format!(
            "plan has been edited since generation: {}",
            parts.join(", ")
        )
    }
}

impl PlanFile {
    /// Build the artifact from labelled transactions, canonicalizing JSON args.
    pub fn new(
        network: String,
        spec_digest: String,
        derived: Derived,
        checks: Vec<Check>,
        steps: Vec<(String, PlannedTransaction)>,
    ) -> anyhow::Result<Self> {
        let steps = steps
            .into_iter()
            .map(|(label, transaction)| PlanStep::from_planned(label, transaction))
            .collect::<anyhow::Result<Vec<_>>>()?;
        let step_digests = steps
            .iter()
            .map(PlanStep::digest)
            .collect::<anyhow::Result<Vec<_>>>()?;

        Ok(Self {
            schema: PLAN_SCHEMA_VERSION,
            tool_version: env!("CARGO_PKG_VERSION").to_owned(),
            network,
            spec_digest,
            step_digests,
            derived,
            checks,
            steps,
        })
    }

    /// Recompute each step's digest and report how the file differs from what
    /// was generated.
    ///
    /// Reported, never enforced. Editing the plan is the feature; refusing an
    /// edited plan would block exactly the case this artifact exists for.
    pub fn drift(&self) -> anyhow::Result<Drift> {
        let current = self
            .steps
            .iter()
            .map(PlanStep::digest)
            .collect::<anyhow::Result<Vec<_>>>()?;

        let changed = current
            .iter()
            .zip(&self.step_digests)
            .enumerate()
            .filter_map(|(index, (now, then))| (now != then).then_some(index))
            .collect();

        Ok(Drift {
            changed,
            added: current.len().saturating_sub(self.step_digests.len()),
            removed: self.step_digests.len().saturating_sub(current.len()),
        })
    }

    /// The transactions this plan will send.
    pub fn into_operation_plan(self) -> anyhow::Result<OperationPlan> {
        Ok(OperationPlan {
            steps: self
                .steps
                .into_iter()
                .map(PlanStep::into_planned)
                .collect::<anyhow::Result<Vec<_>>>()?,
        })
    }
}

/// `sha256:…` over a value's canonical JSON encoding.
pub fn digest(value: &impl Serialize) -> anyhow::Result<String> {
    use sha2::{Digest as _, Sha256};

    let bytes = serde_json::to_vec(value).context("serialize for digest")?;
    Ok(format!("sha256:{}", hex::encode(Sha256::digest(&bytes))))
}
