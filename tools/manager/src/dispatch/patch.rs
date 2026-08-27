use futures::{stream, StreamExt, TryStreamExt};
use std::path::Path;

use anyhow::Context as _;
use near_api::types::{
    transaction::{actions::Action, SignedTransaction, Transaction, TransactionV0},
    NearToken, PublicKey,
};
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
        plan::digest,
    },
};

const PATCH_WASM_VERSION: &str = "0.1.0";
const MAX_STORAGE_KEY_LENGTH: usize = 2_048;
const MAX_STORAGE_VALUE_LENGTH: usize = 4 * 1024 * 1024;
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
            &args.plan,
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

    let resolved = expand_prefixes(ctx, &spec.account_id, spec.resolve(source_path)?).await?;
    let (patch, unguarded, longest_key, longest_value) =
        compile_patch(&spec.account_id, resolved.clone())?;
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
            longest_key <= MAX_STORAGE_KEY_LENGTH,
            format!("largest key is {longest_key} bytes; limit is {MAX_STORAGE_KEY_LENGTH}"),
        ),
    ));
    reporter.record(Check::new(
        "patch.value_length",
        status(
            longest_value <= MAX_STORAGE_VALUE_LENGTH,
            format!("largest value is {longest_value} bytes; limit is {MAX_STORAGE_VALUE_LENGTH}"),
        ),
    ));

    let state_increase = storage_increase(ctx, &spec.account_id, &patch).await?;
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
    let patch_wasm_sha256 = digest(&patch_wasm);
    let patch_args = Base64Bytes(borsh::to_vec(&patch).context("encode PatchState arguments")?);
    let (restore, restore_action, restored_code_len, restore_hash_status) =
        if let Some(hash) = account.global_contract_hash.as_deref() {
            let hash = hash
                .parse::<near_api::types::CryptoHash>()
                .context("parse target global contract hash")?;
            (
                RestoreCode::GlobalCodeHash {
                    hash: Sha256Digest(hash.0),
                },
                ActionInput::UseGlobalContract {
                    contract_identifier: GlobalContractIdentifierInput::CodeHash(CryptoHash::from(
                        hash,
                    )),
                },
                0,
                Status::passed("restore preserves the target global code hash"),
            )
        } else if let Some(account_id) = account.global_contract_account_id.clone() {
            (
                RestoreCode::GlobalAccount {
                    account_id: account_id.clone(),
                },
                ActionInput::UseGlobalContract {
                    contract_identifier: GlobalContractIdentifierInput::AccountId(account_id),
                },
                0,
                Status::passed("restore preserves the target global code account"),
            )
        } else {
            let code = ctx
                .client
                .read(account::GetCode {
                    account_id: spec.account_id.clone(),
                })
                .await?
                .code;
            let code_hash = near_api::types::CryptoHash::hash(&code.0);
            (
                RestoreCode::Local {
                    code_hash: Sha256Digest(code_hash.0),
                },
                ActionInput::DeployContract { code: code.clone() },
                code.len(),
                status(
                    code_hash.to_string() == account.code_hash,
                    format!(
                        "account.get reports {}, fetched code hashes to {code_hash}",
                        account.code_hash
                    ),
                ),
            )
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
    reporter.record(Check::new(
        "patch.storage_balance",
        status(
            account.amount.as_yoctonear() >= required_storage,
            format!(
                "{state_increase} state bytes and {} temporary code bytes require \
                 {required_storage} yoctoNEAR",
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
    let limits = ctx
        .client
        .read(templar_gateway_methods_spec::chain::GetProtocolLimits)
        .await?;
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
    reporter.record(Check::new(
        "patch.action_count",
        status(
            batch.actions.len() <= 100,
            format!("{} actions; limit is 100", batch.actions.len()),
        ),
    ));
    reporter.record(Check::new(
        "patch.code_hash",
        status(
            !restore_hash_status.is_failure() && restore_identity_ok,
            "live restore identity and fetched local code match the reviewed plan",
        ),
    ));

    Ok(PatchPlan {
        schema: PATCH_PLAN_SCHEMA_VERSION,
        tool_version: env!("CARGO_PKG_VERSION").to_owned(),
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
                        prefix,
                    })
                    .await?;
                Ok::<Vec<ResolvedOperation>, templar_gateway_core::GatewayError>(
                    entries
                        .values
                        .into_iter()
                        .map(|entry| ResolvedOperation::Remove {
                            key: entry.key,
                            expected: Some(ResolvedExpectation::Bytes(entry.value)),
                        })
                        .collect(),
                )
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

async fn storage_increase(
    ctx: &CliContext,
    account_id: &near_account_id::AccountId,
    patch: &Patch,
) -> anyhow::Result<usize> {
    let increases = stream::iter(patch.ops.iter().filter_map(|operation| {
        let Op::Set { key, value } = operation else {
            return None;
        };
        let key = key.clone();
        let value = value.clone();
        Some(async move {
            let entries = ctx
                .client
                .read(account::ViewState {
                    account_id: account_id.clone(),
                    prefix: Base64Bytes(key.clone()),
                })
                .await?;
            let previous = entries
                .values
                .into_iter()
                .find(|entry| entry.key.0 == key)
                .map(|entry| entry.value.0);
            Ok::<_, templar_gateway_core::GatewayError>(match previous {
                Some(previous) => value.len().saturating_sub(previous.len()),
                None => key
                    .len()
                    .saturating_add(value.len())
                    .saturating_add(STORAGE_RECORD_OVERHEAD),
            })
        })
    }))
    .buffered(STATE_READ_CONCURRENCY)
    .try_collect::<Vec<usize>>()
    .await?;
    Ok(increases.into_iter().fold(0, usize::saturating_add))
}
fn compile_patch(
    account_id: &near_account_id::AccountId,
    patch: crate::spec::patch::ResolvedPatch,
) -> anyhow::Result<(Patch, bool, usize, usize)> {
    let mut ops = Vec::new();
    let mut unguarded = false;
    let mut longest_key = 0;
    let mut longest_value = 0;
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
        peak_storage_bytes, restore_identity_matches, restore_mode_matches,
        signed_transaction_wire_size, status, total_prepaid_gas,
    };
    use crate::spec::patch::Sha256Digest;
    use crate::spec::patch_plan::RestoreCode;
    use near_account_id::AccountId;
    use near_api::{types::transaction::PrepopulateTransaction, SecretKey, Signer};
    use templar_gateway_methods_spec::tx;
    use templar_gateway_types::{
        common::ContractArgs, ActionInput, Base64Bytes, ContractMethodName, NearGas, NearToken,
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
            code_hash: Sha256Digest([1; 32]),
        };
        let other_local = RestoreCode::Local {
            code_hash: Sha256Digest([2; 32]),
        };
        let global = RestoreCode::GlobalCodeHash {
            hash: Sha256Digest([1; 32]),
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
}
