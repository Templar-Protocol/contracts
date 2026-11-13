use std::path::PathBuf;

use anyhow::{Context, Result};
use near_api::types::AccountId;
use serde::Serialize;
use templar_gateway_methods_spec::{account, contract, tx};
use templar_gateway_types::{common::ContractArgs, ManagedAccountId};

use crate::{
    commands::patch::DryRun,
    context::{print_json, CliContext},
    dispatch::{
        patch::build,
        sandbox_replay::{
            build_local_client, reset_account_metadata, setup_account, stage_snapshot_code,
            start_sandbox,
        },
    },
    spec::{
        check::{gate, Check, Status},
        patch_plan::{DryRunStamp, PatchPlan, RestoreCode, PATCH_PLAN_SCHEMA_VERSION},
        plan::WireSha256Digest,
    },
};

#[derive(Debug, Serialize)]
struct DryRunReport {
    account_id: AccountId,
    sandbox_chain_id: String,
    target_code_hash: templar_gateway_types::CryptoHash,
    state_digest: WireSha256Digest,
    transaction: tx::Batch,
    views: Vec<ViewCheckReport>,
    checks: Vec<Check>,
}

#[derive(Debug, Serialize)]
struct ViewCheckReport {
    id: String,
    method_name: templar_gateway_types::ContractMethodName,
    before: ViewOutcome,
    after: ViewOutcome,
    diff: Option<json_patch::Patch>,
    verdict: Status,
}

#[derive(Debug, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
enum ViewOutcome {
    Returned { value: serde_json::Value },
    Failed { error: String },
}

#[allow(
    clippy::too_many_lines,
    reason = "dry-run orchestration preserves the reviewed batch, reports, and bound stamp in one flow"
)]
pub(super) async fn dry_run(ctx: CliContext, args: DryRun) -> Result<()> {
    reject_non_skippable(&args.skip_check)?;
    let mut plan = load_plan(&args.plan)?;
    let mut reporter = ctx.reporter(&args.skip_check);
    let built = build(
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
    ensure_plan_matches(&plan, &built.plan)?;

    anyhow::ensure!(
        built.plan.state_digest == plan.state_digest,
        "the patch plan state digest does not match the finalized planning snapshot"
    );

    let (sandbox, local_network) = start_sandbox().await?;
    let secret_key = setup_account(
        &local_network,
        &plan.spec.account_id,
        Some(&plan.public_key),
        &built.state,
    )
    .await?;
    let local_client = build_local_client(&local_network, &[(&plan.spec.account_id, &secret_key)])?;
    stage_snapshot_code(
        &local_network,
        &plan.spec.account_id,
        &built.state,
        &secret_key,
    )
    .await?;
    reset_account_metadata(&local_network, &plan.spec.account_id, &built.state).await?;
    anyhow::ensure!(
        restore_matches(&local_client, &plan).await?,
        "sandbox staging did not reproduce the reviewed code/linkage"
    );
    let sandbox_chain_id = local_network.network_name.clone();

    let views_before = evaluate_views(&local_client, &plan).await;
    let execution = local_client
        .execute_as(
            ManagedAccountId(plan.spec.account_id.clone()),
            plan.batch.clone(),
        )
        .await;
    let batch_status = execution_status(&execution);
    let views_after = evaluate_views(&local_client, &plan).await;
    let views = merge_views(&plan, &views_before, &views_after)?;
    let restored = restore_matches(&local_client, &plan).await.unwrap_or(false);
    let dry_status = if batch_status.0 && restored {
        Status::passed("sandbox batch succeeded and reviewed code/linkage was restored")
    } else {
        Status::failed(format!(
            "sandbox batch {}, restore {}{}",
            batch_status.1,
            if restored { "matched" } else { "did not match" },
            execution
                .as_ref()
                .err()
                .map(|error| format!("; {error}"))
                .unwrap_or_default()
        ))
    };
    for view in &views {
        reporter.record(Check::new(view.id.clone(), view.verdict.clone()));
    }
    reporter.record(Check::new("patch.dry_run", dry_status));
    reporter.ensure_every_skip_matched()?;
    reporter.digest();
    let checks = reporter.checks().to_vec();
    let report = DryRunReport {
        account_id: plan.spec.account_id.clone(),
        sandbox_chain_id,
        target_code_hash: plan.target_code_hash,
        state_digest: plan.state_digest,
        transaction: plan.batch.clone(),
        views,
        checks: checks.clone(),
    };
    print_json(&report)?;

    gate(
        &checks,
        plan.spec.account_id.as_str(),
        "patch dry-run did not pass",
    )?;
    let stamp = DryRunStamp {
        plan_digest: plan.unstamped_digest()?,
        sandbox_chain_id: report.sandbox_chain_id.clone(),
        target_code_hash: plan.target_code_hash,
        state_digest: plan.state_digest,
        checks,
    };
    plan.dry_run = Some(stamp);
    write_plan(&args.plan, &plan)?;
    drop(sandbox);
    Ok(())
}

fn reject_non_skippable(skip: &[String]) -> Result<()> {
    for id in skip {
        anyhow::ensure!(
            id != "patch.state_complete" && id != "patch.dry_run",
            "{id} is non-skippable; use the dedicated --no-dry-run override for apply"
        );
    }
    Ok(())
}

fn load_plan(path: &PathBuf) -> Result<PatchPlan> {
    let text = std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    let plan: PatchPlan =
        serde_json::from_str(&text).with_context(|| format!("parse {}", path.display()))?;
    anyhow::ensure!(
        plan.schema == PATCH_PLAN_SCHEMA_VERSION,
        "patch plan schema {} is unsupported; re-run `patch plan`",
        plan.schema
    );
    plan.spec.validate_view_checks()?;
    Ok(plan)
}

fn ensure_plan_matches(plan: &PatchPlan, rederived: &PatchPlan) -> Result<()> {
    let mut expected = plan.clone();
    expected.dry_run = None;
    expected.checks.clear();
    let mut actual = rederived.clone();
    actual.dry_run = None;
    actual.checks.clear();
    anyhow::ensure!(
        serde_json::to_vec(&expected)? == serde_json::to_vec(&actual)?,
        "the patch plan no longer matches its embedded spec and live target; re-run `patch plan`"
    );
    Ok(())
}

async fn evaluate_views(
    client: &templar_gateway_client::Client,
    plan: &PatchPlan,
) -> Vec<Result<serde_json::Value, String>> {
    let mut outcomes = Vec::new();
    for check in plan.spec.view_checks() {
        let result = client
            .read(contract::ViewFunction {
                contract_id: plan.spec.account_id.clone(),
                method_name: check.method_name.clone(),
                args: ContractArgs::Json(check.args.clone()),
            })
            .await
            .map_err(|error| error.to_string());
        outcomes.push(result);
    }
    outcomes
}

fn merge_views(
    plan: &PatchPlan,
    before: &[Result<serde_json::Value, String>],
    after: &[Result<serde_json::Value, String>],
) -> Result<Vec<ViewCheckReport>> {
    plan.spec
        .view_checks()
        .enumerate()
        .map(|(index, check)| {
            let before_outcome = outcome(before[index].clone());
            let after_outcome = outcome(after[index].clone());
            let diff = match (&before[index], &after[index]) {
                (Ok(before), Ok(after)) => Some(json_patch::diff(before, after)),
                _ => None,
            };
            let verdict = view_verdict(check.expected()?, &after[index]);
            Ok(ViewCheckReport {
                id: format!("patch.view.{index}"),
                method_name: check.method_name.clone(),
                before: before_outcome,
                after: after_outcome,
                diff,
                verdict,
            })
        })
        .collect()
}

fn view_verdict(
    expect: Option<&serde_json::Value>,
    after: &Result<serde_json::Value, String>,
) -> Status {
    match after {
        Err(error) => Status::failed(format!("after view failed: {error}")),
        Ok(value) => match expect {
            Some(expect) if expect != value => Status::failed("after view did not match expect"),
            Some(_) | None => Status::passed("after view returned successfully"),
        },
    }
}

fn outcome(result: Result<serde_json::Value, String>) -> ViewOutcome {
    match result {
        Ok(value) => ViewOutcome::Returned { value },
        Err(error) => ViewOutcome::Failed { error },
    }
}

fn execution_status(
    result: &Result<
        templar_gateway_types::common::WriteOperationResult,
        templar_gateway_core::GatewayError,
    >,
) -> (bool, String) {
    match result {
        Ok(result) => {
            let success = matches!(
                result.operation.status,
                templar_gateway_types::OperationStatus::Succeeded
            );
            let failures = result
                .operation
                .steps
                .iter()
                .filter_map(|step| step.status.outcome()?.failure.as_deref())
                .collect::<Vec<_>>()
                .join("; ");
            (
                success,
                if failures.is_empty() {
                    format!("{:?}", result.operation.status)
                } else {
                    failures
                },
            )
        }
        Err(error) => (false, format!("failed: {error}")),
    }
}

async fn restore_matches(
    client: &templar_gateway_client::Client,
    plan: &PatchPlan,
) -> Result<bool> {
    let account = client
        .read(account::Get {
            account_id: plan.spec.account_id.clone(),
        })
        .await?;
    Ok(match &plan.restore {
        RestoreCode::Local { .. } => account.code_hash == plan.target_code_hash.to_string(),
        RestoreCode::GlobalCodeHash { hash } => {
            account.global_contract_hash.as_deref() == Some(hash.to_string().as_str())
        }
        RestoreCode::GlobalAccount { account_id } => {
            account.global_contract_account_id.as_ref() == Some(account_id)
        }
    })
}

fn write_plan(path: &PathBuf, plan: &PatchPlan) -> Result<()> {
    let rendered = serde_json::to_string_pretty(plan).context("render dry-run stamp")?;
    std::fs::write(path, format!("{rendered}\n"))
        .with_context(|| format!("write dry-run stamp to {}", path.display()))
}
#[cfg(test)]
mod tests {
    use super::view_verdict;
    use crate::spec::check::Status;

    #[test]
    fn view_check_expectation_gates_after_state_only() {
        let expect: serde_json::Value = serde_json::from_str(r#"{"version":1}"#).unwrap();
        assert_eq!(
            view_verdict(
                Some(&expect),
                &Ok(serde_json::from_str(r#"{"version":1}"#).unwrap()),
            ),
            Status::Passed {
                detail: "after view returned successfully".to_owned()
            }
        );
        assert!(view_verdict(
            Some(&expect),
            &Ok(serde_json::from_str(r#"{"version":2}"#).unwrap()),
        )
        .is_failure());
        assert!(
            !view_verdict(None, &Ok(serde_json::from_str(r#"{"version":2}"#).unwrap()),)
                .is_failure()
        );
        assert!(view_verdict(Some(&expect), &Err("before only".to_owned())).is_failure());
    }

    #[test]
    fn json_text_expectations_compare_exact_nullable_values() {
        use crate::spec::patch::{JsonText, PatchViewCheck};
        use serde_json::json;

        for expected in [json!(null), json!({"maximum":null})] {
            let check = PatchViewCheck {
                method_name: "get_configuration".to_owned().into(),
                args: json!({}),
                expect: None,
                expect_json: Some(JsonText::from_value(expected.clone()).unwrap()),
            };
            assert!(!view_verdict(check.expected().unwrap(), &Ok(expected)).is_failure());
            assert!(view_verdict(check.expected().unwrap(), &Ok(json!("null"))).is_failure());
        }
    }
}
