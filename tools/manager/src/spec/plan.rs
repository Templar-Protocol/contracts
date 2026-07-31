//! The plan artifact: a serializable, hand-editable form of an
//! [`OperationPlan`], for when the spec cannot express what a market needs or a
//! check is wrong.
//!
//! ## Why JSON args are canonicalized
//!
//! `near_api` serializes `FunctionCallAction::args` as base64, which is useless
//! in a file someone has to edit. Carrying them as JSON means re-encoding
//! through [`serde_json::Value`], which sorts object keys the gateway emitted in
//! declaration order — so the original bytes cannot be preserved. Conversion
//! into this file therefore canonicalizes, and the canonical form is what gets
//! digested, displayed, and executed: what `apply` sends is what `plan` showed.
//! Key order means nothing to a contract decoding with `serde_json`.

use anyhow::Context as _;
use near_account_id::AccountId;
use near_api::types::transaction::actions::{Action, FunctionCallAction};
use near_api::types::NearToken;
use serde::{Deserialize, Serialize};
use templar_gateway_core::{OperationPlan, PlannedTransaction};
use templar_gateway_types::{Base64Bytes, ManagedAccountId, NearGas};

use super::check::Check;

/// Bumped when this artifact's shape changes. `apply` hard-refuses a mismatch:
/// every struct here is `deny_unknown_fields`, and this file authorizes spending
/// real NEAR.
pub const PLAN_SCHEMA_VERSION: u32 = 1;

/// A function call's arguments, in whichever form a human can actually edit.
///
/// Deliberately not [`templar_gateway_types::common::ContractArgs`], which
/// models the same choice but serializes as `{"encoding": …, "value": …}`. This
/// file is read and edited by hand, so the terser `{"json": …}` is the shape
/// worth having.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlanArgs {
    Json(serde_json::Value),
    Base64(Base64Bytes),
}

impl PlanArgs {
    /// Classify by probing the bytes rather than by a list of method names,
    /// which would drift from the methods it describes. Only a JSON *object*
    /// counts — every NEAR contract takes its JSON args as one, so requiring it
    /// rules out borsh that happens to parse as a bare scalar.
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
    /// Gas units, as a plain integer so editing needs no knowledge of
    /// `NearGas`'s parse format.
    pub gas: u64,
    pub deposit: NearToken,
}

/// One transaction. Every step a deployment plans is a function call; any other
/// action kind is refused at conversion rather than silently dropped.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanStep {
    /// Manager-only, so a diff of an edited plan is legible. The executor never
    /// reads it, which is why it lives here and not in [`PlannedTransaction`].
    pub label: String,
    pub signer_id: AccountId,
    pub receiver_id: AccountId,
    #[serde(default)]
    pub continue_on_failure: bool,
    pub function_calls: Vec<PlanFunctionCall>,
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
                     planned {other:?}"
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
}

/// Values the spec implies rather than states, so a reviewer can see what the
/// tool concluded without re-deriving them.
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
    /// Digest of the spec this plan came from, so a plan can be traced back to
    /// its source after the spec has moved on.
    pub spec_digest: String,
    /// Per-step digests as generated. One per step rather than a single
    /// whole-file digest: with seven steps, "something changed" is not
    /// actionable.
    pub step_digests: Vec<String>,
    pub derived: Derived,
    pub checks: Vec<Check>,
    pub steps: Vec<PlanStep>,
}

/// What changed between a plan as generated and the file as it stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Drift {
    /// Indices whose content differs from the recorded digest.
    pub changed: Vec<usize>,
    /// Steps added (positive) or removed (negative).
    pub delta: isize,
}

impl Drift {
    pub fn is_clean(&self) -> bool {
        self.changed.is_empty() && self.delta == 0
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
        match self.delta {
            0 => {}
            added if added > 0 => parts.push(format!("{added} step(s) added")),
            removed => parts.push(format!("{} step(s) removed", -removed)),
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
            .map(digest)
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

    /// How the file differs from what was generated.
    ///
    /// Reported, never enforced: editing the plan is the feature, so refusing an
    /// edited plan would block the case this artifact exists for.
    pub fn drift(&self) -> anyhow::Result<Drift> {
        let current = self
            .steps
            .iter()
            .map(digest)
            .collect::<anyhow::Result<Vec<_>>>()?;

        Ok(Drift {
            changed: current
                .iter()
                .zip(&self.step_digests)
                .enumerate()
                .filter_map(|(index, (now, then))| (now != then).then_some(index))
                .collect(),
            delta: isize::try_from(current.len()).unwrap_or(isize::MAX)
                - isize::try_from(self.step_digests.len()).unwrap_or(isize::MAX),
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

/// `sha256:…` over a value's JSON encoding.
pub fn digest(value: &impl Serialize) -> anyhow::Result<String> {
    let bytes = serde_json::to_vec(value).context("serialize for digest")?;
    Ok(format!(
        "sha256:{}",
        templar_contract_artifacts::sha256_hex(&bytes)
    ))
}
