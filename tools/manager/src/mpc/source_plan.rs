//! Strict review boundary shared by ordinary DAO proposals and MPC execution.
//! Ordinary provenance is SHA-256 over the domain below followed by Borsh
//! `(signer AccountId, receiver AccountId, ordered Vec<Action>, failure bool)`.
//! It includes every action byte, not the function-call-only deployment journal.

use std::path::Path;

use anyhow::{ensure, Context as _, Result};
use near_api::types::transaction::actions::Action;
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest as _, Sha256};
use templar_gateway_core::PlannedTransaction;
use templar_gateway_types::{protocol::MAX_ACTIONS_PER_RECEIPT, ManagedAccountId};

use crate::spec::{
    check::Status,
    patch::ResolvedOperation,
    patch_plan::{PatchPlan, PATCH_PLAN_SCHEMA_VERSION},
    plan::WireSha256Digest,
};

use super::payload::Decoded;

const TRANSACTION_DIGEST_DOMAIN: &[u8] = b"tmplrmgr:source-plan:planned-transaction:v1\0";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum SourcePlanKind {
    PlannedTransaction,
    PatchPlan,
}

pub(crate) struct SourcePlan {
    transaction: PlannedTransaction,
    patch: Option<Box<PatchPlan>>,
    digest: WireSha256Digest,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct StrictTransaction {
    signer_account_id: ManagedAccountId,
    receiver_id: near_account_id::AccountId,
    actions: Vec<Action>,
    continue_on_failure: bool,
}

impl From<StrictTransaction> for PlannedTransaction {
    fn from(value: StrictTransaction) -> Self {
        Self {
            signer_account_id: value.signer_account_id,
            receiver_id: value.receiver_id,
            actions: value.actions,
            continue_on_failure: value.continue_on_failure,
        }
    }
}

fn strict_decode<T: DeserializeOwned + Serialize>(value: &Value) -> Result<T> {
    let decoded = T::deserialize(value).context("decode source plan")?;
    ensure!(
        serde_json::to_value(&decoded)? == *value,
        "source plan contains unknown fields or noncanonical values"
    );
    Ok(decoded)
}

impl SourcePlan {
    pub(crate) fn load(path: &Path) -> Result<Self> {
        Self::from_value(&crate::commands::load_json_file(path)?)
    }

    pub(crate) fn from_value(value: &Value) -> Result<Self> {
        let object = value
            .as_object()
            .context("source plan must be one JSON object")?;
        let (transaction, patch, digest) = match (
            object.contains_key("schema"),
            object.contains_key("signer_account_id"),
        ) {
            (false, true) => {
                let transaction: PlannedTransaction = strict_decode::<StrictTransaction>(value)?.into();
                let digest = transaction_digest(&transaction)?;
                (transaction, None, digest)
            }
            (true, false) => {
                let plan: PatchPlan = strict_decode(value)?;
                ensure!(plan.schema == PATCH_PLAN_SCHEMA_VERSION, "unsupported PatchPlan schema {}", plan.schema);
                ensure!(!plan.unguarded, "MPC requires a guarded PatchPlan");
                plan.spec.validate_view_checks()?;
                let digest = plan.unstamped_digest()?;
                let stamp = plan.dry_run.as_ref().context("MPC requires a stamped PatchPlan; run patch dry-run")?;
                ensure!(
                    stamp.plan_digest == digest && stamp.state_digest == plan.state_digest
                        && stamp.target_code_hash == plan.target_code_hash,
                    "PatchPlan dry-run stamp does not bind this source plan"
                );
                ensure!(
                    !stamp.checks.is_empty() && stamp.checks.iter().all(|check| matches!(check.status, Status::Passed { .. })),
                    "MPC requires every PatchPlan replay check to have passed"
                );
                ensure!(
                    plan.signer_id == plan.spec.account_id && plan.batch.receiver_id == plan.spec.account_id,
                    "PatchPlan signer and batch receiver must equal its target"
                );
                ensure!(!plan.resolved.operations.is_empty(), "PatchPlan contains no operations");
                let transaction = PlannedTransaction::new(
                    ManagedAccountId(plan.signer_id.clone()),
                    plan.batch.receiver_id.clone(),
                    plan.batch.actions.iter().cloned().map(Action::try_from).collect::<Result<Vec<_>, _>>()?,
                );
                validate_patch_shape(&plan, &transaction)?;
                (transaction, Some(Box::new(plan)), digest)
            }
            _ => anyhow::bail!("expected exactly one PlannedTransaction or schema-3 PatchPlan; ambiguous and multi-step plans are not accepted"),
        };
        ensure!(
            !transaction.continue_on_failure,
            "source plan cannot continue_on_failure"
        );
        ensure!(
            !transaction.actions.is_empty() && transaction.actions.len() <= MAX_ACTIONS_PER_RECEIPT,
            "source plan must contain 1..={MAX_ACTIONS_PER_RECEIPT} ordered actions"
        );
        Ok(Self {
            transaction,
            patch,
            digest,
        })
    }

    pub(crate) fn transaction(&self) -> &PlannedTransaction {
        &self.transaction
    }

    pub(crate) fn patch(&self) -> Option<&PatchPlan> {
        self.patch.as_deref()
    }

    pub(crate) fn kind(&self) -> SourcePlanKind {
        if self.patch.is_some() {
            SourcePlanKind::PatchPlan
        } else {
            SourcePlanKind::PlannedTransaction
        }
    }

    pub(crate) fn digest(&self) -> WireSha256Digest {
        self.digest
    }

    pub(crate) fn into_transaction(self) -> PlannedTransaction {
        self.transaction
    }

    pub(crate) fn ensure_matches(&self, decoded: &Decoded) -> Result<()> {
        ensure!(
            decoded.signer_id == self.transaction.signer_account_id.0
                && decoded.receiver_id == self.transaction.receiver_id
                && decoded.actions == self.transaction.actions,
            "approved MPC signer, receiver or ordered action bytes differ from the source plan"
        );
        if let Some(plan) = self.patch() {
            ensure!(
                decoded.public_key == plan.public_key,
                "approved MPC key differs from the PatchPlan key"
            );
        }
        Ok(())
    }
}

pub(crate) fn load_ordinary(path: &Path) -> Result<SourcePlan> {
    let plan = SourcePlan::load(path)?;
    ensure!(
        plan.kind() == SourcePlanKind::PlannedTransaction,
        "ordinary DAO proposals require one PlannedTransaction, not a PatchPlan"
    );
    Ok(plan)
}

pub(crate) fn transaction_digest(plan: &PlannedTransaction) -> Result<WireSha256Digest> {
    let mut hash = Sha256::new();
    hash.update(TRANSACTION_DIGEST_DOMAIN);
    borsh::to_writer(
        &mut hash,
        &(
            &plan.signer_account_id.0,
            &plan.receiver_id,
            &plan.actions,
            plan.continue_on_failure,
        ),
    )?;
    Ok(WireSha256Digest(hash.finalize().into()))
}

fn validate_patch_shape(plan: &PatchPlan, transaction: &PlannedTransaction) -> Result<()> {
    let [Action::DeployContract(_), Action::FunctionCall(call), restore] =
        transaction.actions.as_slice()
    else {
        anyhow::bail!("PatchPlan must be one atomic deploy, patch, restore batch");
    };
    ensure!(
        call.method_name == "patch"
            && call.deposit.is_zero()
            && matches!(
                restore,
                Action::DeployContract(_) | Action::UseGlobalContract(_)
            ),
        "PatchPlan has an invalid patch call or restore action"
    );
    let patch = templar_patch_state_types::Patch::from_borsh_slice(&call.args)
        .map_err(|error| anyhow::anyhow!("invalid PatchPlan patch payload: {error:?}"))?;
    let expected_count = plan
        .resolved
        .operations
        .iter()
        .try_fold(0usize, |count, operation| {
            let increment = match operation {
                ResolvedOperation::Set { expected, .. }
                | ResolvedOperation::Remove { expected, .. } => 1 + usize::from(expected.is_some()),
                ResolvedOperation::Expect { .. } => 1,
                ResolvedOperation::RemovePrefix { .. } => {
                    anyhow::bail!("PatchPlan contains an unresolved prefix removal")
                }
            };
            count
                .checked_add(increment)
                .context("PatchPlan operation count overflow")
        })?;
    ensure!(
        patch.account_id == plan.spec.account_id.as_str() && patch.ops.len() == expected_count,
        "PatchPlan patch target or operation count differs from its resolved operations"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use near_api::types::transaction::actions::{
        DeployContractAction, FunctionCallAction, TransferAction,
    };
    use templar_gateway_types::{NearGas, NearToken};

    fn transaction() -> PlannedTransaction {
        PlannedTransaction::new(
            ManagedAccountId("dao.near".parse().unwrap()),
            "target.near".parse().unwrap(),
            vec![
                Action::DeployContract(DeployContractAction {
                    code: vec![0, 97, 115, 109],
                }),
                Action::FunctionCall(Box::new(FunctionCallAction {
                    method_name: "execute".to_owned(),
                    args: vec![0, 255, 42],
                    gas: NearGas::from_tgas(20),
                    deposit: NearToken::from_yoctonear(7),
                })),
                Action::Transfer(TransferAction {
                    deposit: NearToken::from_yoctonear(8),
                }),
            ],
        )
    }

    #[test]
    fn strict_boundary_rejects_extra_fields_ambiguous_shapes_and_failure_policy() {
        let original = serde_json::to_value(transaction()).unwrap();
        let mut extra = original.clone();
        extra["unreviewed"] = serde_json::json!(true);
        let mut ambiguous = original.clone();
        ambiguous["schema"] = serde_json::json!(3);
        let mut nested = original.clone();
        let action = nested["actions"][1]
            .as_object_mut()
            .unwrap()
            .values_mut()
            .next()
            .unwrap();
        action["unreviewed"] = serde_json::json!(true);
        let mut tolerant = original.clone();
        tolerant["continue_on_failure"] = serde_json::json!(true);
        let mut empty = original.clone();
        empty["actions"] = serde_json::json!([]);
        for invalid in [
            extra,
            ambiguous,
            nested,
            tolerant,
            empty,
            serde_json::json!({"steps":[original.clone()]}),
        ] {
            assert!(SourcePlan::from_value(&invalid).is_err());
        }
        assert_eq!(
            SourcePlan::from_value(&original)
                .unwrap()
                .into_transaction(),
            transaction()
        );
    }

    #[test]
    fn ordinary_digest_and_decoded_binding_cover_full_ordered_actions_identities_gas_and_deposit() {
        let original = transaction();
        let digest = transaction_digest(&original).unwrap();
        // These wire digests are persisted in reviewed source-plan provenance.
        assert_eq!(
            serde_json::to_value(digest).unwrap(),
            serde_json::json!(
                "sha256-wire:f49e86f2cceed33af103c7499354caf687e968bbaa4e8b157555feb4a3a89597"
            )
        );
        let mut large = transaction();
        let Action::DeployContract(deploy) = &mut large.actions[0] else {
            panic!("deployment fixture");
        };
        deploy.code = vec![0xa5; 16 * 1024];
        assert_eq!(
            serde_json::to_value(transaction_digest(&large).unwrap()).unwrap(),
            serde_json::json!(
                "sha256-wire:b426832c5ae8396b71e83ba1f9c7a6a2c88d3113c4d9debbe880a6d16a75f486"
            )
        );
        let decoded = Decoded {
            kind: crate::mpc::payload::PayloadKind::DelegateAction,
            signer_id: original.signer_account_id.0.clone(),
            receiver_id: original.receiver_id.clone(),
            actions: original.actions.clone(),
            public_key: near_api::PublicKey::empty(near_api::types::crypto::KeyType::ED25519),
            nonce: 1,
            validity: crate::mpc::payload::Validity::MaxBlockHeight(100),
        };
        SourcePlan::from_value(&serde_json::to_value(&original).unwrap())
            .unwrap()
            .ensure_matches(&decoded)
            .unwrap();
        let mut variants = Vec::new();
        let mut changed = original.clone();
        changed.signer_account_id = ManagedAccountId("other.near".parse().unwrap());
        variants.push(changed);
        let mut changed = original.clone();
        changed.receiver_id = "other.near".parse().unwrap();
        variants.push(changed);
        let mut changed = original.clone();
        changed.actions.swap(0, 1);
        variants.push(changed);
        let mut changed = original.clone();
        if let Action::DeployContract(action) = &mut changed.actions[0] {
            action.code.push(1);
        }
        variants.push(changed);
        for field in 0..4 {
            let mut changed = original.clone();
            if let Action::FunctionCall(action) = &mut changed.actions[1] {
                match field {
                    0 => action.method_name.push('x'),
                    1 => action.args.push(1),
                    2 => action.gas = NearGas::from_tgas(21),
                    _ => action.deposit = NearToken::from_yoctonear(9),
                }
            }
            variants.push(changed);
        }
        let mut changed = original.clone();
        changed.continue_on_failure = true;
        assert_ne!(transaction_digest(&changed).unwrap(), digest);
        for changed in variants {
            assert_ne!(transaction_digest(&changed).unwrap(), digest);
            assert!(
                SourcePlan::from_value(&serde_json::to_value(changed).unwrap())
                    .unwrap()
                    .ensure_matches(&decoded)
                    .is_err()
            );
        }
    }
}
