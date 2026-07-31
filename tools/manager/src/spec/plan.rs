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
pub const PLAN_SCHEMA_VERSION: u32 = 2;

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
            Ok(value) if value.is_object() && exactly_representable(&value) => Self::Json(value),
            _ => Self::Base64(Base64Bytes(args)),
        }
    }

    /// The bytes this will send.
    ///
    /// Borrowing rather than consuming, because the collision check has to read
    /// the same bytes the executor will. Re-checks representability: `from_bytes`
    /// runs at generation, but it is the *edited* file that gets sent, and a
    /// hand-written number too large for `u64` would otherwise be re-encoded in
    /// exponent form — a value nobody wrote.
    pub(crate) fn to_bytes(&self) -> anyhow::Result<Vec<u8>> {
        match self {
            Self::Json(value) => {
                anyhow::ensure!(
                    exactly_representable(value),
                    "these args carry a number that cannot be re-encoded exactly; \
                     it would be sent in a different form than written"
                );
                serde_json::to_vec(value).context("encode JSON args")
            }
            Self::Base64(bytes) => Ok(bytes.0.clone()),
        }
    }
}

/// Whether every number survives a decode/re-encode unchanged.
///
/// `serde_json` without `arbitrary_precision` demotes an integer too large for
/// `u64` to `f64`, which re-encodes in exponent form — a different value than
/// the operator reviewed, on a transaction that spends real money. Today every
/// large number reaching these args is a string (`U128`, `Decimal`,
/// `NearToken`), so nothing hits this; the classifier is method-agnostic by
/// design, so the first one that does must fall back to opaque bytes rather
/// than be silently rewritten.
fn exactly_representable(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::Number(number) => number.is_i64() || number.is_u64(),
        serde_json::Value::Array(items) => items.iter().all(exactly_representable),
        serde_json::Value::Object(fields) => fields.values().all(exactly_representable),
        _ => true,
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
    /// In yoctoNEAR (1 NEAR = 10^24). `render` prints the human form; this is
    /// the raw value, and editing it by eye is how you send 6 yoctoNEAR.
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

        // Not carried in the artifact. A tolerated revert still completes the
        // operation as `Succeeded`, so an editable `continue_on_failure` would
        // let a hand-edit turn a reverted governance call into `apply` exiting
        // zero. Nothing a deployment plans sets it, so it is refused rather
        // than rendered.
        anyhow::ensure!(
            !transaction.continue_on_failure,
            "`{label}` tolerates its own failure, which a deployment plan must \
             never do"
        );

        Ok(Self {
            label,
            signer_id: transaction.signer_account_id.0,
            receiver_id: transaction.receiver_id,
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
                    args: call.args.to_bytes()?,
                    gas: NearGas::from_gas(call.gas),
                    deposit: call.deposit,
                })))
            })
            .collect::<anyhow::Result<Vec<_>>>()?;

        Ok(PlannedTransaction {
            signer_account_id: ManagedAccountId::from(self.signer_id),
            receiver_id: self.receiver_id,
            actions,
            continue_on_failure: false,
        })
    }
}

/// Values the spec implies rather than states, so a reviewer can see what the
/// tool concluded without re-deriving them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Derived {
    /// Whether this deployment creates its own proxy oracle.
    ///
    /// Recorded rather than inferred. The shape of a direct plan — one deploy,
    /// no proposals — is indistinguishable from a proxy plan whose proxy steps
    /// were deleted, and three attempts to tell them apart from account names
    /// were each wrong in a different way. It is covered by `summary_digest`,
    /// so editing it is reported as drift like any other claim in the file.
    ///
    /// Not `#[serde(default)]`: a plan written before this field existed would
    /// default to `false`, read as "direct", and skip the completeness
    /// requirement that is the entire point — a defaulted bool defaults to the
    /// unsafe answer. The schema version is bumped instead, so such a file is
    /// refused outright rather than misread.
    pub creates_its_own_oracle: bool,
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
    /// Digest over everything this file states except the steps themselves —
    /// provenance, derived ids, check results. Without it, editing a check from
    /// `failed` to `passed`, repointing `derived.market_id`, or rewriting the
    /// `spec_digest` that `render` presents as the plan's source all report the
    /// plan as unmodified.
    pub summary_digest: String,
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
    /// The derived values or check results differ.
    pub summary: bool,
}

impl Drift {
    pub fn is_clean(&self) -> bool {
        self.changed.is_empty() && self.delta == 0 && !self.summary
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
        if self.summary {
            parts.push("the derived values or check results differ".to_owned());
        }
        format!(
            "plan has been edited since generation: {}",
            parts.join(", ")
        )
    }
}

impl PlanFile {
    /// Build the artifact from labelled transactions, canonicalizing JSON args.
    #[cfg(test)]
    pub fn new(
        network: String,
        spec_digest: String,
        derived: Derived,
        checks: Vec<Check>,
        steps: Vec<(String, PlannedTransaction)>,
    ) -> anyhow::Result<Self> {
        let steps = Self::steps_from(steps)?;
        Self::from_steps(network, spec_digest, derived, checks, steps)
    }

    /// The artifact's steps, converted but not yet sealed into a file — so a
    /// check that has to read them (funding, ENG-545) can run before the digests
    /// that must cover its result are computed.
    pub fn steps_from(steps: Vec<(String, PlannedTransaction)>) -> anyhow::Result<Vec<PlanStep>> {
        steps
            .into_iter()
            .map(|(label, transaction)| PlanStep::from_planned(label, transaction))
            .collect()
    }

    pub fn from_steps(
        network: String,
        spec_digest: String,
        derived: Derived,
        checks: Vec<Check>,
        steps: Vec<PlanStep>,
    ) -> anyhow::Result<Self> {
        let step_digests = steps
            .iter()
            .map(digest)
            .collect::<anyhow::Result<Vec<_>>>()?;
        let summary_digest = summary_digest(
            PLAN_SCHEMA_VERSION,
            env!("CARGO_PKG_VERSION"),
            &network,
            &spec_digest,
            &derived,
            &checks,
        )?;

        Ok(Self {
            schema: PLAN_SCHEMA_VERSION,
            tool_version: env!("CARGO_PKG_VERSION").to_owned(),
            network,
            spec_digest,
            step_digests,
            summary_digest,
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
            summary: summary_digest(
                self.schema,
                &self.tool_version,
                &self.network,
                &self.spec_digest,
                &self.derived,
                &self.checks,
            )? != self.summary_digest,
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

fn summary_digest(
    schema: u32,
    tool_version: &str,
    network: &str,
    spec_digest: &str,
    derived: &Derived,
    checks: &[Check],
) -> anyhow::Result<String> {
    digest(&(schema, tool_version, network, spec_digest, derived, checks))
}

/// `sha256:…` over a value's *canonical* JSON encoding.
///
/// Canonical because a spec's `yield_weights.static` is a `HashMap`, whose
/// iteration order is randomized per process — hashing the plain encoding gave
/// the same spec a different `spec_digest` on every invocation, which is the
/// opposite of the traceability the field exists for.
pub fn digest(value: &impl Serialize) -> anyhow::Result<String> {
    let bytes = serde_json_canonicalizer::to_vec(value).context("serialize for digest")?;
    Ok(format!(
        "sha256:{}",
        templar_contract_artifacts::sha256_hex(&bytes)
    ))
}

#[cfg(test)]
pub mod testing {
    use near_api::types::NearToken;
    use templar_gateway_methods_spec::account;

    /// An `account.get` result with only the fields the funding check reads.
    pub fn account(amount: NearToken, locked: NearToken, storage_usage: u64) -> account::GetResult {
        account::GetResult {
            amount,
            locked,
            code_hash: String::new(),
            storage_usage,
            global_contract_hash: None,
            global_contract_account_id: None,
        }
    }
}
