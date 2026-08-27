use futures::{stream, StreamExt, TryStreamExt};
use std::path::Path;

use anyhow::Context as _;
use near_api::types::{
    transaction::{actions::Action, SignedTransaction, Transaction, TransactionV0},
    NearToken, PublicKey,
};
use sha2::{Digest as _, Sha256};
use templar_contract_artifacts::{fetch, ArtifactId};
use templar_gateway_methods_spec::{account, tx};
use templar_gateway_types::{
    common::ContractArgs,
    primitive::{CryptoHash, GlobalContractIdentifierInput},
    ActionInput, Base64Bytes, NearGas,
};
use templar_patch_state_types::{Op, Patch};

use crate::{
    commands::{
        patch::{Apply, Plan},
        registry::STORAGE_AMOUNT_PER_BYTE,
    },
    context::CliContext,
    report::Reporter,
    spec::{
        check::{gate, Check, Status},
        patch::{ResolvedExpectation, ResolvedOperation, Sha256Digest},
        patch_plan::{PatchPlan, RestoreCode, PATCH_PLAN_SCHEMA_VERSION},
    },
};

const PATCH_WASM_VERSION: &str = "0.1.0";
const PATCH_GAS: NearGas = NearGas::from_tgas(300);
const STORAGE_RECORD_OVERHEAD: usize = 40;
const STATE_READ_CONCURRENCY: usize = 8;

pub(super) async fn plan(ctx: CliContext, args: Plan) -> anyhow::Result<()> {
    let spec = crate::spec::patch::PatchSpec::load(&args.path)?;
    let mut reporter = ctx.reporter(&args.skip_check);
    let plan = build(
        &ctx,
        &args.path,
        spec,
        args.signer_id,
        args.public_key,
        args.allow_unguarded,
        None,
        &mut reporter,
    )
    .await?;
    reporter.ensure_every_skip_matched()?;
    gate(
        reporter.checks(),
        plan.spec.account_id.as_str(),
        "no patch plan was written",
    )?;
    reporter.digest();

    let rendered = serde_json::to_string_pretty(&plan).context("render patch plan")?;
    match args.out {
        Some(path) => {
            std::fs::write(&path, format!("{rendered}\n"))
                .with_context(|| format!("write {}", path.display()))?;
            eprintln!("Wrote patch plan to {}", path.display());
        }
        None => println!("{rendered}"),
    }
    Ok(())
}

pub(super) async fn apply(ctx: CliContext, args: Apply) -> anyhow::Result<()> {
    let plan: PatchPlan = {
        let text = std::fs::read_to_string(&args.plan)
            .with_context(|| format!("read {}", args.plan.display()))?;
        serde_json::from_str(&text).with_context(|| format!("parse {}", args.plan.display()))?
    };
    anyhow::ensure!(
        plan.schema == PATCH_PLAN_SCHEMA_VERSION,
        "patch plan schema {} is unsupported; re-run `patch plan`",
        plan.schema
    );
    anyhow::ensure!(
        args.signer.account_id().0 == plan.signer_id,
        "patch plan expects signer `{}`, but apply uses `{}`",
        plan.signer_id,
        args.signer.account_id().0,
    );

    let public_key = args.signer.public_key()?;
    anyhow::ensure!(
        public_key == templar_gateway_types::primitive::PublicKey::from(plan.public_key),
        "patch plan was reviewed for a different signing public key"
    );
    let mut reporter = ctx.reporter(&args.skip_check);
    {
        let rederived = build(
            &ctx,
            &plan.source_path,
            plan.spec.clone(),
            plan.signer_id.clone(),
            plan.public_key,
            args.allow_unguarded,
            Some(&plan.restore),
            &mut reporter,
        )
        .await?;
        anyhow::ensure!(
            rederived.batch == plan.batch
                && rederived.patch_wasm_sha256 == plan.patch_wasm_sha256
                && rederived.resolved == plan.resolved
                && rederived.restore == plan.restore,
            "the patch plan no longer matches its embedded spec and live target; re-run `patch plan`"
        );
    }
    reporter.ensure_every_skip_matched()?;
    gate(
        reporter.checks(),
        plan.spec.account_id.as_str(),
        "patch was not sent",
    )?;
    reporter.digest();
    ctx.write(args.signer, plan.batch).await
}

fn ensure_patch_signer(
    signer_id: &near_account_id::AccountId,
    target_id: &near_account_id::AccountId,
) -> anyhow::Result<()> {
    anyhow::ensure!(
        signer_id == target_id,
        "patch signer `{signer_id}` must equal target `{target_id}`"
    );
    Ok(())
}

fn restore_from_account(account: &account::GetResult) -> anyhow::Result<RestoreCode> {
    if let Some(hash) = account.global_contract_hash.as_deref() {
        let hash = hash
            .parse::<near_api::types::CryptoHash>()
            .context("parse target global contract hash")?;
        return Ok(RestoreCode::GlobalCodeHash {
            hash: CryptoHash::from(hash),
        });
    }
    if let Some(account_id) = account.global_contract_account_id.clone() {
        return Ok(RestoreCode::GlobalAccount { account_id });
    }

    anyhow::ensure!(
        account.code_hash != near_api::types::CryptoHash([0; 32]).to_string(),
        "patch target has no deployed contract code to restore"
    );
    let code_hash = account
        .code_hash
        .parse::<near_api::types::CryptoHash>()
        .context("parse target local code hash")?;
    Ok(RestoreCode::Local {
        code_hash: CryptoHash::from(code_hash),
    })
}
fn patch_wasm_digest(bytes: &[u8]) -> Sha256Digest {
    Sha256Digest(Sha256::digest(bytes).into())
}

#[allow(
    clippy::too_many_lines,
    clippy::too_many_arguments,
    reason = "planning derives one checked atomic transaction from one spec"
)]
async fn build(
    ctx: &CliContext,
    source_path: &Path,
    spec: crate::spec::patch::PatchSpec,
    signer_id: near_account_id::AccountId,
    public_key: near_api::PublicKey,
    allow_unguarded: bool,
    expected_restore: Option<&RestoreCode>,
    reporter: &mut Reporter,
) -> anyhow::Result<PatchPlan> {
    ensure_patch_signer(&signer_id, &spec.account_id)?;
    let source_path = source_path
        .canonicalize()
        .with_context(|| format!("canonicalize patch source {}", source_path.display()))?;
    let declared_network = crate::spec::network_for_account(&spec.account_id)?;
    reporter.record(Check::new(
        "patch.network",
        status(
            declared_network == ctx.network(),
            format!(
                "target is {declared_network}, selected network is {}",
                ctx.network()
            ),
        ),
    ));

    let account = ctx
        .client
        .read(account::Get {
            account_id: spec.account_id.clone(),
        })
        .await?;
    let limits = ctx
        .client
        .read(templar_gateway_methods_spec::chain::GetProtocolLimits)
        .await?;

    let resolved = expand_prefixes(ctx, &spec.account_id, spec.resolve(&source_path)?).await?;
    let (patch, unguarded, longest_key, longest_value, state_increase) =
        compile_patch(&spec.account_id, resolved.clone())?;
    let longest_key = u64::try_from(longest_key).context("patch key length exceeds u64")?;
    let longest_value = u64::try_from(longest_value).context("patch value length exceeds u64")?;
    reporter.record(Check::new(
        "patch.expectations",
        status(
            allow_unguarded || !unguarded,
            "every set/remove must declare an expectation or use --allow-unguarded",
        ),
    ));
    reporter.record(Check::new(
        "patch.key_length",
        status(
            longest_key <= limits.max_length_storage_key,
            format!(
                "largest key is {longest_key} bytes; limit is {}",
                limits.max_length_storage_key
            ),
        ),
    ));
    reporter.record(Check::new(
        "patch.value_length",
        status(
            longest_value <= limits.max_length_storage_value,
            format!(
                "largest value is {longest_value} bytes; limit is {}",
                limits.max_length_storage_value
            ),
        ),
    ));

    let access = ctx
        .client
        .read(account::GetAccessKey {
            account_id: spec.account_id.clone(),
            public_key: public_key.into(),
        })
        .await?;
    reporter.record(Check::new(
        "patch.full_access_key",
        status(
            matches!(access.permission, account::AccessKeyPermission::FullAccess),
            "signer key must have full access on the patch target",
        ),
    ));

    let patch_wasm = fetch::released_bytes(ArtifactId::PatchState, PATCH_WASM_VERSION)
        .await
        .context("load the pinned PatchState release")?;
    let patch_wasm_sha256 = patch_wasm_digest(&patch_wasm);
    let patch_args = Base64Bytes(borsh::to_vec(&patch).context("encode PatchState arguments")?);
    let restore = restore_from_account(&account)?;
    let (restore_action, restored_code_len, restore_hash_status) = match &restore {
        RestoreCode::GlobalCodeHash { hash } => (
            ActionInput::UseGlobalContract {
                contract_identifier: GlobalContractIdentifierInput::CodeHash(*hash),
            },
            0,
            Status::passed("restore preserves the target global code hash"),
        ),
        RestoreCode::GlobalAccount { account_id } => (
            ActionInput::UseGlobalContract {
                contract_identifier: GlobalContractIdentifierInput::AccountId(account_id.clone()),
            },
            0,
            Status::passed("restore preserves the target global code account"),
        ),
        RestoreCode::Local { code_hash } => {
            let code = ctx
                .client
                .read(account::GetCode {
                    account_id: spec.account_id.clone(),
                })
                .await?
                .code;
            let fetched_hash = CryptoHash::from(near_api::types::CryptoHash::hash(&code.0));
            (
                ActionInput::DeployContract { code: code.clone() },
                code.len(),
                status(
                    fetched_hash == *code_hash,
                    format!(
                        "account.get reports {code_hash}, fetched code hashes to {fetched_hash}"
                    ),
                ),
            )
        }
    };
    let restore_mode_ok =
        expected_restore.is_none_or(|expected| restore_mode_matches(expected, &restore));
    let restore_identity_ok =
        expected_restore.is_none_or(|expected| restore_identity_matches(expected, &restore));
    reporter.record(Check::new(
        "patch.restore_mode",
        status(
            restore_mode_ok,
            "live restore mode matches the reviewed patch plan",
        ),
    ));
    let required_storage = peak_storage_bytes(
        account.storage_usage,
        state_increase,
        patch_wasm.len(),
        restored_code_len,
    )
    .saturating_mul(STORAGE_AMOUNT_PER_BYTE.as_yoctonear());
    let liquid_storage = account.amount.as_yoctonear();
    let locked_storage = account.locked.as_yoctonear();
    let available_storage = liquid_storage
        .checked_add(locked_storage)
        .context("sum account storage backing")?;
    reporter.record(Check::new(
        "patch.storage_balance",
        status(
            available_storage >= required_storage,
            format!(
                "{state_increase} state bytes and {} temporary code bytes require \
                 {required_storage} yoctoNEAR; liquid {liquid_storage}, locked \
                 {locked_storage}, available {available_storage}",
                patch_wasm.len().saturating_sub(restored_code_len)
            ),
        ),
    ));

    let batch = tx::Batch {
        receiver_id: spec.account_id.clone(),
        actions: vec![
            ActionInput::DeployContract {
                code: Base64Bytes(patch_wasm),
            },
            ActionInput::FunctionCall {
                method_name: "patch".to_owned().into(),
                args: ContractArgs::Raw(patch_args),
                gas: PATCH_GAS,
                deposit: NearToken::from_yoctonear(0),
            },
            restore_action,
        ],
    };
    let prepaid_gas = total_prepaid_gas(&batch)?;
    reporter.record(Check::new(
        "patch.gas",
        status(
            prepaid_gas <= limits.max_total_prepaid_gas,
            format!(
                "{} prepaid gas; live limit is {}",
                prepaid_gas.as_gas(),
                limits.max_total_prepaid_gas.as_gas()
            ),
        ),
    ));
    let wire_size = signed_transaction_wire_size(&signer_id, public_key, &batch)?;
    reporter.record(Check::new(
        "patch.tx_size",
        status(
            wire_size as u64 <= limits.max_transaction_size,
            format!(
                "{wire_size} bytes; live limit is {}",
                limits.max_transaction_size
            ),
        ),
    ));
    let code_hash_status = if restore_hash_status.is_failure() {
        restore_hash_status
    } else if restore_identity_ok {
        Status::passed("live restore identity and fetched local code match the reviewed plan")
    } else {
        Status::failed(format!(
            "reviewed restore identity {}, live restore identity {}",
            serde_json::to_string(
                expected_restore.context("restore identity check has no reviewed restore")?
            )?,
            serde_json::to_string(&restore)?,
        ))
    };
    reporter.record(Check::new("patch.code_hash", code_hash_status));
    Ok(PatchPlan {
        schema: PATCH_PLAN_SCHEMA_VERSION,
        tool_version: env!("CARGO_PKG_VERSION").to_owned(),
        source_path,
        spec,
        resolved,
        signer_id,
        public_key,
        patch_wasm_sha256,
        restore,
        batch,
        unguarded,
        checks: reporter.checks().to_vec(),
    })
}
async fn expand_prefixes(
    ctx: &CliContext,
    account_id: &near_account_id::AccountId,
    patch: crate::spec::patch::ResolvedPatch,
) -> anyhow::Result<crate::spec::patch::ResolvedPatch> {
    let groups = stream::iter(patch.operations.into_iter().map(|operation| async {
        match operation {
            ResolvedOperation::RemovePrefix { prefix } => {
                let entries = ctx
                    .client
                    .read(account::ViewState {
                        account_id: account_id.clone(),
                        prefix: prefix.clone(),
                    })
                    .await?;
                expand_prefix_entries(&prefix, entries.values)
            }
            operation => Ok(vec![operation]),
        }
    }))
    .buffered(STATE_READ_CONCURRENCY)
    .try_collect::<Vec<Vec<ResolvedOperation>>>()
    .await?
    .into_iter()
    .flatten()
    .collect();
    Ok(crate::spec::patch::ResolvedPatch { operations: groups })
}

fn expand_prefix_entries(
    prefix: &Base64Bytes,
    entries: Vec<account::StateEntry>,
) -> anyhow::Result<Vec<ResolvedOperation>> {
    anyhow::ensure!(
        !entries.is_empty(),
        "remove_prefix {} matched no storage keys",
        serde_json::to_string(prefix)?
    );
    Ok(entries
        .into_iter()
        .map(|entry| ResolvedOperation::Remove {
            key: entry.key,
            expected: Some(ResolvedExpectation::Bytes(entry.value)),
        })
        .collect())
}

fn set_storage_increase(
    key: &Base64Bytes,
    value: &Base64Bytes,
    expected: Option<&ResolvedExpectation>,
) -> usize {
    match expected {
        Some(ResolvedExpectation::Bytes(previous)) => {
            value.0.len().saturating_sub(previous.0.len())
        }
        Some(ResolvedExpectation::Hash(_)) => value.0.len(),
        Some(ResolvedExpectation::Absent) | None => key
            .0
            .len()
            .saturating_add(value.0.len())
            .saturating_add(STORAGE_RECORD_OVERHEAD),
    }
}

fn compile_patch(
    account_id: &near_account_id::AccountId,
    patch: crate::spec::patch::ResolvedPatch,
) -> anyhow::Result<(Patch, bool, usize, usize, usize)> {
    let mut ops = Vec::new();
    let mut unguarded = false;
    let mut longest_key = 0;
    let mut longest_value = 0;
    let mut state_increase: usize = 0;
    for operation in patch.operations {
        match operation {
            ResolvedOperation::Expect { key, expected } => {
                longest_key = longest_key.max(key.len());
                ops.push(expect_op(key.0, expected));
            }
            ResolvedOperation::Set {
                key,
                value,
                expected,
            } => {
                longest_key = longest_key.max(key.len());
                longest_value = longest_value.max(value.len());
                state_increase = state_increase.saturating_add(set_storage_increase(
                    &key,
                    &value,
                    expected.as_ref(),
                ));
                if let Some(expected) = expected {
                    ops.push(expect_op(key.0.clone(), expected));
                } else {
                    unguarded = true;
                }
                ops.push(Op::Set {
                    key: key.0,
                    value: value.0,
                });
            }
            ResolvedOperation::Remove { key, expected } => {
                longest_key = longest_key.max(key.len());
                if let Some(expected) = expected {
                    ops.push(expect_op(key.0.clone(), expected));
                } else {
                    unguarded = true;
                }
                ops.push(Op::Remove { key: key.0 });
            }
            ResolvedOperation::RemovePrefix { .. } => {
                anyhow::bail!("prefix deletes must be expanded before compiling the patch")
            }
        }
    }
    Ok((
        Patch {
            account_id: account_id.to_string(),
            ops,
        },
        unguarded,
        longest_key,
        longest_value,
        state_increase,
    ))
}

fn expect_op(key: Vec<u8>, expected: ResolvedExpectation) -> Op {
    match expected {
        ResolvedExpectation::Bytes(value) => Op::Expect {
            key,
            value: Some(value.0),
        },
        ResolvedExpectation::Hash(sha256) => Op::ExpectHash {
            key,
            sha256: sha256.0,
        },
        ResolvedExpectation::Absent => Op::Expect { key, value: None },
    }
}

fn restore_mode_matches(expected: &RestoreCode, actual: &RestoreCode) -> bool {
    std::mem::discriminant(expected) == std::mem::discriminant(actual)
}

fn restore_identity_matches(expected: &RestoreCode, actual: &RestoreCode) -> bool {
    expected == actual
}

fn peak_storage_bytes(
    storage_usage: u64,
    state_increase: usize,
    patch_wasm_len: usize,
    restored_code_len: usize,
) -> u128 {
    u128::from(storage_usage)
        .saturating_add(state_increase as u128)
        .saturating_add(patch_wasm_len.saturating_sub(restored_code_len) as u128)
}

fn total_prepaid_gas(batch: &tx::Batch) -> anyhow::Result<NearGas> {
    batch
        .actions
        .iter()
        .try_fold(NearGas::from_gas(0), |total, action| {
            let ActionInput::FunctionCall { gas, .. } = action else {
                return Ok(total);
            };
            total
                .checked_add(*gas)
                .context("sum function-call prepaid gas")
        })
}

fn signed_transaction_wire_size(
    signer_id: &near_account_id::AccountId,
    public_key: PublicKey,
    batch: &tx::Batch,
) -> anyhow::Result<usize> {
    let actions = batch
        .actions
        .iter()
        .cloned()
        .map(Action::try_from)
        .collect::<Result<Vec<_>, _>>()
        .context("convert patch actions")?;
    let signature_len = match public_key {
        PublicKey::ED25519(_) => 64,
        PublicKey::SECP256K1(_) => 65,
    };
    let signature =
        near_api::types::Signature::from_parts(public_key.key_type(), &vec![0; signature_len])
            .context("construct placeholder signature")?;

    let transaction = Transaction::V0(TransactionV0 {
        signer_id: signer_id.clone(),
        public_key,
        nonce: 0,
        receiver_id: batch.receiver_id.clone(),
        block_hash: near_api::types::CryptoHash([0; 32]),
        actions,
    });
    borsh::to_vec(&SignedTransaction::new(signature, transaction))
        .context("encode signed transaction")
        .map(|bytes| bytes.len())
}

fn status(passed: bool, detail: impl Into<String>) -> Status {
    if passed {
        Status::passed(detail)
    } else {
        Status::failed(detail)
    }
}
#[cfg(test)]
mod tests {
    use super::{
        compile_patch, ensure_patch_signer, expand_prefix_entries, peak_storage_bytes,
        restore_from_account, restore_identity_matches, restore_mode_matches, set_storage_increase,
        signed_transaction_wire_size, status, total_prepaid_gas, Op,
    };
    use crate::spec::patch::{ResolvedExpectation, ResolvedOperation, ResolvedPatch, Sha256Digest};
    use crate::spec::patch_plan::RestoreCode;
    use near_account_id::AccountId;
    use near_api::{types::transaction::PrepopulateTransaction, SecretKey, Signer};
    use templar_gateway_methods_spec::{account, tx};
    use templar_gateway_types::{
        common::ContractArgs, ActionInput, Base64Bytes, ContractMethodName, CryptoHash, NearGas,
        NearToken,
    };

    fn batch() -> tx::Batch {
        tx::Batch {
            receiver_id: "receiver.near".parse().unwrap(),
            actions: vec![ActionInput::FunctionCall {
                method_name: ContractMethodName("method".to_owned()),
                args: ContractArgs::Raw(Base64Bytes(b"args".to_vec())),
                gas: NearGas::from_tgas(1),
                deposit: NearToken::from_yoctonear(0),
            }],
        }
    }

    #[tokio::test]
    async fn signed_wire_size_matches_real_signed_transaction() {
        let secret: SecretKey =
            "ed25519:2vVTQWpoZvYZBS4HYFZtzU2rxpoQSrhyFWdaHLqSdyaEfgjefbSKiFpuVatuRqax3HFvVq2tkkqWH2h7tso2nK8q"
                .parse()
                .unwrap();
        let signer = Signer::from_secret_key(secret.clone()).unwrap();
        let signer_id: AccountId = "signer.near".parse().unwrap();
        let batch = batch();
        let actions = batch
            .actions
            .iter()
            .cloned()
            .map(near_api::types::transaction::actions::Action::try_from)
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        let signed_transaction = signer
            .sign(
                PrepopulateTransaction {
                    signer_id: signer_id.clone(),
                    receiver_id: batch.receiver_id.clone(),
                    actions,
                },
                secret.public_key(),
                0,
                near_api::types::CryptoHash([0; 32]),
            )
            .await
            .unwrap();
        let size = signed_transaction_wire_size(&signer_id, secret.public_key(), &batch).unwrap();
        assert_eq!(size, borsh::to_vec(&signed_transaction).unwrap().len());
        assert!(!status(size <= size, "at limit").is_failure());
        assert!(status(size.saturating_add(1) <= size, "over limit").is_failure());
    }

    #[tokio::test]
    async fn signed_wire_size_matches_real_secp256k1_transaction() {
        let secret: SecretKey = "secp256k1:11111111111111111111111111111112"
            .parse()
            .unwrap();
        let signer = Signer::from_secret_key(secret.clone()).unwrap();
        let signer_id: AccountId = "signer.near".parse().unwrap();
        let batch = batch();
        let actions = batch
            .actions
            .iter()
            .cloned()
            .map(near_api::types::transaction::actions::Action::try_from)
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        let signed_transaction = signer
            .sign(
                PrepopulateTransaction {
                    signer_id: signer_id.clone(),
                    receiver_id: batch.receiver_id.clone(),
                    actions,
                },
                secret.public_key(),
                0,
                near_api::types::CryptoHash([0; 32]),
            )
            .await
            .unwrap();
        assert_eq!(
            signed_transaction_wire_size(&signer_id, secret.public_key(), &batch).unwrap(),
            borsh::to_vec(&signed_transaction).unwrap().len()
        );
    }

    #[test]
    fn restore_checks_distinguish_modes_and_identifiers() {
        let local = RestoreCode::Local {
            code_hash: CryptoHash::from(near_api::types::CryptoHash([1; 32])),
        };
        let other_local = RestoreCode::Local {
            code_hash: CryptoHash::from(near_api::types::CryptoHash([2; 32])),
        };
        let global = RestoreCode::GlobalCodeHash {
            hash: CryptoHash::from(near_api::types::CryptoHash([1; 32])),
        };
        assert!(restore_mode_matches(&local, &other_local));
        assert!(!restore_identity_matches(&local, &other_local));
        assert!(!restore_mode_matches(&local, &global));
        assert!(!restore_identity_matches(&local, &global));
    }

    #[test]
    fn total_prepaid_gas_sums_function_calls() {
        let mut batch = batch();
        batch.actions.push(batch.actions[0].clone());
        assert_eq!(total_prepaid_gas(&batch).unwrap(), NearGas::from_tgas(2));
    }

    #[test]
    fn peak_storage_counts_temporary_code_delta() {
        assert_eq!(peak_storage_bytes(100, 10, 30, 20), 120);
        assert_eq!(peak_storage_bytes(100, 10, 20, 30), 110);
        assert_eq!(peak_storage_bytes(100, 10, 20, 20), 110);
        assert_eq!(peak_storage_bytes(100, 10, 20, 0), 130);
    }
    #[test]
    fn signer_must_match_target() {
        let signer: AccountId = "signer.near".parse().unwrap();
        let target: AccountId = "target.near".parse().unwrap();
        let error = ensure_patch_signer(&signer, &target).expect_err("different signer rejected");
        assert_eq!(
            error.to_string(),
            "patch signer `signer.near` must equal target `target.near`"
        );
    }

    #[test]
    fn absent_expectation_is_guarded_and_has_record_bound() {
        let key = Base64Bytes(b"key".to_vec());
        let value = Base64Bytes(b"value".to_vec());
        assert_eq!(
            set_storage_increase(&key, &value, Some(&ResolvedExpectation::Absent)),
            48
        );
        let (patch, unguarded, _, _, state_increase) = compile_patch(
            &"target.near".parse().unwrap(),
            ResolvedPatch {
                operations: vec![ResolvedOperation::Set {
                    key,
                    value,
                    expected: Some(ResolvedExpectation::Absent),
                }],
            },
        )
        .unwrap();
        assert!(!unguarded);
        assert_eq!(state_increase, 48);
        assert!(matches!(patch.ops[0], Op::Expect { value: None, .. }));
    }

    #[test]
    fn storage_estimates_use_expectation_bounds() {
        let key = Base64Bytes(b"key".to_vec());
        let value = Base64Bytes(b"value".to_vec());
        assert_eq!(
            set_storage_increase(
                &key,
                &value,
                Some(&ResolvedExpectation::Bytes(Base64Bytes(b"old".to_vec())))
            ),
            2
        );
        assert_eq!(
            set_storage_increase(
                &key,
                &value,
                Some(&ResolvedExpectation::Hash(Sha256Digest([0; 32])))
            ),
            value.len()
        );
        assert_eq!(set_storage_increase(&key, &value, None), 48);
    }

    #[test]
    fn empty_prefix_expansion_is_rejected() {
        let prefix = Base64Bytes(b"prefix".to_vec());
        let error = expand_prefix_entries(&prefix, Vec::new()).expect_err("empty prefix rejected");
        assert_eq!(
            error.to_string(),
            r#"remove_prefix "cHJlZml4" matched no storage keys"#
        );
    }

    #[test]
    fn codeless_account_cannot_be_restored_as_local_code() {
        let account = account::GetResult {
            amount: NearToken::from_yoctonear(0),
            locked: NearToken::from_yoctonear(0),
            code_hash: near_api::types::CryptoHash([0; 32]).to_string(),
            storage_usage: 0,
            global_contract_hash: None,
            global_contract_account_id: None,
        };
        let error = restore_from_account(&account).expect_err("codeless account rejected");
        assert_eq!(
            error.to_string(),
            "patch target has no deployed contract code to restore"
        );
    }
}
