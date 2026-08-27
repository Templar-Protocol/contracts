//! The plan artifact: a serializable form of an [`OperationPlan`], carrying the
//! spec it was derived from. `apply` re-derives and refuses a file that does not
//! match, so a plan cannot say something its spec does not.
//!
//! JSON args are canonicalized on conversion, since re-encoding through
//! [`serde_json::Value`] cannot preserve the gateway's key order. The canonical
//! form is what gets displayed and executed, so what `apply` sends is what
//! `plan` showed.

use anyhow::Context as _;
use clap::ValueEnum;
use near_account_id::AccountId;
use near_api::types::transaction::actions::{Action, FunctionCallAction};
use near_api::types::NearToken;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use sha2::{Digest as _, Sha256};
use templar_gateway_core::{OperationPlan, PlannedTransaction};
use templar_gateway_types::{primitive::PublicKey, Base64Bytes, ManagedAccountId, NearGas};

use super::check::Check;

/// `apply` hard-refuses a mismatch: every struct here is `deny_unknown_fields`,
/// and this file authorizes spending real NEAR.
///
/// Still 1, and stays 1 until the tool ships: no plan written by any build has
/// left a developer's machine, so a bump would distinguish shapes that exist
/// nowhere. `the_plan_shape_is_pinned_to_its_version` fails when the shape
/// changes, which is the point at which that stops being true.
pub const PLAN_SCHEMA_VERSION: u32 = 2;

/// The terminal semantic stage included in a deployment plan.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, ValueEnum)]
#[serde(rename_all = "snake_case")]
#[value(rename_all = "kebab-case")]
pub enum DeploymentStage {
    Governance,
    ProxyOracle,
    ProxyConfiguration,
    Market,
}

impl Default for DeploymentStage {
    fn default() -> Self {
        Self::Market
    }
}

/// A function call's arguments, in whichever form a human can actually read.
/// Not `ContractArgs`, whose `{"encoding": …, "value": …}` buries them.
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

    /// The bytes this will send. Re-checks representability: these came back
    /// through `serde_json`, which is where a number loses precision.
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
/// `serde_json` demotes an integer too large for `u64` to `f64` and re-encodes
/// it in exponent form — a different value than the operator reviewed, on a
/// transaction that spends real money.
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
    /// Gas units.
    pub gas: u64,
    /// In yoctoNEAR (1 NEAR = 10^24); `render` prints the human form.
    pub deposit: NearToken,
}

/// One transaction. Every step a deployment plans is a function call; any other
/// action kind is refused at conversion rather than silently dropped.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanStep {
    /// Manager-only, so a rendered plan is legible. The executor never reads
    /// it, which is why it lives here and not in [`PlannedTransaction`].
    pub label: String,
    pub signer_id: AccountId,
    pub receiver_id: AccountId,
    pub function_calls: Vec<PlanFunctionCall>,
}

impl PlanStep {
    /// Equality over what the executor sends. [`Self::label`] carries a
    /// `(n/total)` counter, so a conditional step dropping out of a re-derivation
    /// renumbers the steps after it without changing what any of them do.
    pub fn executes_the_same_as(&self, other: &Self) -> bool {
        // Destructured so a new field has to be classified here.
        let Self {
            label: _,
            signer_id,
            receiver_id,
            function_calls,
        } = self;
        *signer_id == other.signer_id
            && *receiver_id == other.receiver_id
            && *function_calls == other.function_calls
    }

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

        // It would turn a reverted governance call into `apply` exiting zero.
        // Nothing a deployment plans sets it.
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

/// A plan and the spec it came from.
///
/// `apply` re-derives the steps from `spec` and refuses a file that does not
/// match, so every property a deployment needs — feed coverage, step order,
/// proposal numbering — holds by construction rather than by inspection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanFile {
    pub schema: u32,
    pub tool_version: String,
    /// Fully resolved: `extends` applied, decimals filled in by the preflight.
    /// The network is derived from its registry, so it is not stated twice.
    /// Re-derivation must not depend on files beside the plan.
    pub spec: super::MarketSpec,
    /// The last complete semantic stage authorized by this plan.
    pub stop_after: DeploymentStage,
    /// The key the deploys grant full access to. An input to `build`, so it is
    /// recorded rather than re-supplied at apply time.
    pub public_key: PublicKey,
    pub checks: Vec<Check>,
    pub steps: Vec<PlanStep>,
}

impl PlanFile {
    /// Build the artifact from labeled transactions, canonicalizing JSON args.
    #[cfg(test)]
    pub fn new(
        spec: super::MarketSpec,
        public_key: PublicKey,
        checks: Vec<Check>,
        steps: Vec<(String, PlannedTransaction)>,
    ) -> anyhow::Result<Self> {
        Ok(Self::from_steps(
            spec,
            DeploymentStage::Market,
            public_key,
            checks,
            Self::steps_from(steps)?,
        ))
    }

    /// The artifact's steps, converted but not yet sealed into a file — so a
    /// check that has to read them (funding, ENG-545) can run first, and so
    /// `apply` can convert a re-derived plan for comparison.
    pub fn steps_from(steps: Vec<(String, PlannedTransaction)>) -> anyhow::Result<Vec<PlanStep>> {
        steps
            .into_iter()
            .map(|(label, transaction)| PlanStep::from_planned(label, transaction))
            .collect()
    }

    pub fn from_steps(
        spec: super::MarketSpec,
        stop_after: DeploymentStage,
        public_key: PublicKey,
        checks: Vec<Check>,
        steps: Vec<PlanStep>,
    ) -> Self {
        Self {
            schema: PLAN_SCHEMA_VERSION,
            tool_version: env!("CARGO_PKG_VERSION").to_owned(),
            spec,
            stop_after,
            public_key,
            checks,
            steps,
        }
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

/// Prefix naming what was hashed. Journals persist these, so a change to the
/// encoding must be legible as a format difference rather than as a plan that
/// changed under a completed step.
pub const DIGEST_FORMAT: &str = "sha256-wire";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct WireSha256Digest(pub [u8; 32]);

impl std::fmt::Display for WireSha256Digest {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{DIGEST_FORMAT}:{}", hex::encode(self.0))
    }
}

impl Serialize for WireSha256Digest {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for WireSha256Digest {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        let Some(hex) = value.strip_prefix(&format!("{DIGEST_FORMAT}:")) else {
            return Err(serde::de::Error::custom(format!(
                "digest must start with `{DIGEST_FORMAT}:`"
            )));
        };
        if hex.len() != 64 || hex.bytes().any(|byte| byte.is_ascii_uppercase()) {
            return Err(serde::de::Error::custom(
                "digest must contain exactly 64 lowercase hex characters",
            ));
        }
        let bytes = hex::decode(hex).map_err(serde::de::Error::custom)?;
        let bytes: [u8; 32] = bytes
            .try_into()
            .map_err(|_| serde::de::Error::custom("digest must decode to exactly 32 bytes"))?;
        Ok(Self(bytes))
    }
}

/// `sha256-wire:…` over bytes.
#[must_use]
pub fn digest(bytes: &[u8]) -> WireSha256Digest {
    WireSha256Digest(Sha256::digest(bytes).into())
}

#[cfg(test)]
mod tests {
    use super::{digest, WireSha256Digest};

    #[test]
    fn wire_digest_round_trips_only_its_versioned_encoding() {
        let digest = digest(b"abc");
        assert_eq!(
            serde_json::to_string(&digest).unwrap(),
            r#""sha256-wire:ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad""#
        );
        assert_eq!(
            serde_json::from_str::<WireSha256Digest>(
                r#""sha256-wire:ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad""#
            )
            .unwrap(),
            digest
        );
        assert!(serde_json::from_str::<WireSha256Digest>(
            r#""sha256-json:ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad""#
        )
        .is_err());
        assert!(serde_json::from_str::<WireSha256Digest>(
            r#""sha256-wire:BA7816BF8F01CFEA414140DE5DAE2223B00361A396177A9CB410FF61F20015AD""#
        )
        .is_err());
    }
}

#[cfg(test)]
pub mod testing {
    use near_api::types::NearToken;
    use templar_gateway_methods_spec::account;
    use templar_gateway_types::primitive::PublicKey;

    /// A checked-in proxy-mode spec, for tests that need a real one.
    pub fn alpha_market() -> super::super::MarketSpec {
        crate::spec::extends::load(
            &std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../deployments/alpha/iethfxrp-ixlmusdc.toml"),
        )
        .expect("fixture spec should load")
    }

    pub fn public_key() -> PublicKey {
        PublicKey::from(
            "ed25519:H9k5eiU4xXS3M4z8HzKJSLaZdqGdGwBG49o7orNC4eZW"
                .parse::<near_api::PublicKey>()
                .expect("valid key"),
        )
    }

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
