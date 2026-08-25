use std::path::Path;

use anyhow::Context as _;
use near_api::types::NearToken;
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
        patch::{ResolvedExpectation, ResolvedOperation},
        patch_plan::{PatchPlan, PATCH_PLAN_SCHEMA_VERSION},
        plan::digest,
    },
};

const PATCH_WASM_VERSION: &str = "0.1.0";
const MAX_TRANSACTION_SIZE: usize = 1_572_864;
const MAX_STORAGE_KEY_LENGTH: usize = 2_048;
const MAX_STORAGE_VALUE_LENGTH: usize = 4 * 1024 * 1024;
const PATCH_GAS: NearGas = NearGas::from_tgas(300);
const STORAGE_RECORD_OVERHEAD: usize = 40;

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
        "patch plan expects signer `{}`, but apply uses `{:?}`",
        plan.signer_id,
        args.signer.account_id(),
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
            &mut reporter,
        )
        .await?;
        anyhow::ensure!(
            rederived.batch == plan.batch
                && rederived.patch_wasm_sha256 == plan.patch_wasm_sha256
                && rederived.resolved == plan.resolved,
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
    reason = "planning derives one checked atomic transaction from one spec"
)]
async fn build(
    ctx: &CliContext,
    source_path: &Path,
    spec: crate::spec::patch::PatchSpec,
    signer_id: near_account_id::AccountId,
    public_key: near_api::PublicKey,
    allow_unguarded: bool,
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

    let storage_increase = storage_increase(ctx, &spec.account_id, &patch).await?;
    let required_storage = u128::from(account.storage_usage)
        .saturating_add(storage_increase as u128)
        .saturating_mul(STORAGE_AMOUNT_PER_BYTE.as_yoctonear());
    reporter.record(Check::new(
        "patch.storage_balance",
        status(
            account.amount.as_yoctonear() >= required_storage,
            format!(
                "{storage_increase} additional bytes require final storage backing of \
                 {required_storage} yoctoNEAR"
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
    let patch_wasm_sha256 = digest(&patch_wasm);
    let patch_args = Base64Bytes(borsh::to_vec(&patch).context("encode PatchState arguments")?);
    let (restore, restore_hash) = if let Some(hash) = account.global_contract_hash.as_deref() {
        let hash = hash
            .parse::<near_api::types::CryptoHash>()
            .context("parse target global contract hash")?;
        (
            ActionInput::UseGlobalContract {
                contract_identifier: GlobalContractIdentifierInput::CodeHash(CryptoHash::from(
                    hash,
                )),
            },
            Status::passed("restore preserves the target global code hash"),
        )
    } else if let Some(account_id) = account.global_contract_account_id.clone() {
        (
            ActionInput::UseGlobalContract {
                contract_identifier: GlobalContractIdentifierInput::AccountId(account_id),
            },
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
        let code_hash = near_api::types::CryptoHash::hash(&code.0).to_string();
        (
            ActionInput::DeployContract { code },
            status(
                code_hash == account.code_hash,
                format!(
                    "account.get reports {}, fetched code hashes to {code_hash}",
                    account.code_hash
                ),
            ),
        )
    };
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
            restore,
        ],
    };
    let encoded = serde_json::to_vec(&batch).context("measure patch batch")?;
    reporter.record(Check::new(
        "patch.tx_size",
        status(
            encoded.len() <= MAX_TRANSACTION_SIZE,
            format!("{} bytes; limit is {MAX_TRANSACTION_SIZE}", encoded.len()),
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
        "patch.restore_mode",
        Status::passed("restore action preserves the account's local or global code mode"),
    ));
    reporter.record(Check::new(
        "patch.gas",
        status(
            PATCH_GAS <= NearGas::from_tgas(300),
            "patch call uses at most 300 Tgas",
        ),
    ));
    reporter.record(Check::new("patch.code_hash", restore_hash));

    Ok(PatchPlan {
        schema: PATCH_PLAN_SCHEMA_VERSION,
        tool_version: env!("CARGO_PKG_VERSION").to_owned(),
        spec,
        resolved,
        signer_id,
        public_key,
        patch_wasm_sha256,
        restore_code_hash: account.code_hash,
        global_contract_hash: account.global_contract_hash,
        global_contract_account_id: account.global_contract_account_id,
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
    let mut operations = Vec::new();
    for operation in patch.operations {
        match operation {
            ResolvedOperation::RemovePrefix { prefix } => {
                let entries = ctx
                    .client
                    .read(account::ViewState {
                        account_id: account_id.clone(),
                        prefix: Base64Bytes(prefix),
                    })
                    .await?;
                operations.extend(entries.values.into_iter().map(|entry| {
                    ResolvedOperation::Remove {
                        key: entry.key.0,
                        expected: Some(ResolvedExpectation::Bytes(entry.value.0)),
                    }
                }));
            }
            operation => operations.push(operation),
        }
    }
    Ok(crate::spec::patch::ResolvedPatch { operations })
}

async fn storage_increase(
    ctx: &CliContext,
    account_id: &near_account_id::AccountId,
    patch: &Patch,
) -> anyhow::Result<usize> {
    let mut increase = 0usize;
    for operation in &patch.ops {
        let Op::Set { key, value } = operation else {
            continue;
        };
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
            .find(|entry| entry.key.0 == *key)
            .map(|entry| entry.value.0);
        increase = increase.saturating_add(match previous {
            Some(previous) => value.len().saturating_sub(previous.len()),
            None => key
                .len()
                .saturating_add(value.len())
                .saturating_add(STORAGE_RECORD_OVERHEAD),
        });
    }
    Ok(increase)
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
                ops.push(expect_op(key, expected));
            }
            ResolvedOperation::Set {
                key,
                value,
                expected,
            } => {
                longest_key = longest_key.max(key.len());
                longest_value = longest_value.max(value.len());
                if let Some(expected) = expected {
                    ops.push(expect_op(key.clone(), expected));
                } else {
                    unguarded = true;
                }
                ops.push(Op::Set { key, value });
            }
            ResolvedOperation::Remove { key, expected } => {
                longest_key = longest_key.max(key.len());
                if let Some(expected) = expected {
                    ops.push(expect_op(key.clone(), expected));
                } else {
                    unguarded = true;
                }
                ops.push(Op::Remove { key });
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
            value: Some(value),
        },
        ResolvedExpectation::Hash(sha256) => Op::ExpectHash { key, sha256 },
    }
}

fn status(passed: bool, detail: impl Into<String>) -> Status {
    if passed {
        Status::passed(detail)
    } else {
        Status::failed(detail)
    }
}
