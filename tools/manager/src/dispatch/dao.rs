//! Shared Sputnik submission and the ordinary FunctionCall proposal frontend.

use anyhow::Context as _;
use near_account_id::AccountId;
use serde::Serialize;
use sputnikdao2::ProposalInput;
use templar_gateway_core::PlannedTransaction;
use templar_gateway_methods_spec::dao;
use templar_gateway_types::common::WriteOperationResult;

use crate::commands::{
    dao::{GetProposal, Propose},
    signer::{Authorization, Mode},
};
use crate::context::{check_operation_status, print_json, sputnik_function_call, CliContext};
use crate::mpc::source_plan::load_ordinary;
use crate::spec::plan::WireSha256Digest;

#[derive(Serialize)]
struct ProposeOutput {
    dao: AccountId,
    proposal_id: u64,
    source_plan_digest: WireSha256Digest,
    proposal: ProposalInput,
    add_proposal: WriteOperationResult,
}

pub(super) async fn propose(ctx: CliContext, args: Propose) -> anyhow::Result<()> {
    let source = load_ordinary(&args.plan)?;
    let source_plan_digest = source.digest();
    let proposal = ordinary_proposal(&args.dao, source.into_transaction(), args.description)?;
    let authorization = Authorization::try_from(&args.signer)?;
    let Some(submitted) = submit_proposal(&ctx, authorization, args.dao.clone(), proposal).await?
    else {
        return Ok(());
    };
    print_json(&ProposeOutput {
        dao: args.dao,
        proposal_id: submitted.proposal_id,
        source_plan_digest,
        proposal: submitted.proposal,
        add_proposal: submitted.operation,
    })
}

pub(crate) fn ordinary_proposal(
    dao: &AccountId,
    plan: PlannedTransaction,
    description: String,
) -> anyhow::Result<ProposalInput> {
    anyhow::ensure!(
        plan.signer_account_id.0 == *dao,
        "the source plan signer must be the DAO {dao}"
    );
    anyhow::ensure!(
        !plan.continue_on_failure,
        "DAO proposals do not support continue_on_failure"
    );
    anyhow::ensure!(!plan.actions.is_empty(), "the source plan has no actions");
    Ok(ProposalInput {
        description,
        kind: sputnik_function_call(plan)
            .context("ordinary DAO proposals require FunctionCall-only actions")?,
    })
}

pub(super) async fn get_proposal(ctx: CliContext, args: GetProposal) -> anyhow::Result<()> {
    ctx.read(dao::GetProposal {
        dao_id: args.dao,
        id: args.proposal_id,
    })
    .await
}

pub(super) struct SubmittedProposal {
    pub proposal_id: u64,
    pub proposal: ProposalInput,
    pub operation: WriteOperationResult,
}

/// Submit once, retain the operation identity on every post-submission error, and
/// verify the exact ID returned by Sputnik rather than guessing from a counter.
/// Plan-only mode prints through the ordinary authorized-write path.
pub(super) async fn submit_proposal(
    ctx: &CliContext,
    authorization: Authorization,
    dao_id: AccountId,
    proposal: ProposalInput,
) -> anyhow::Result<Option<SubmittedProposal>> {
    let spec = dao::AddProposal { dao_id, proposal };
    if matches!(authorization.mode(), Mode::Plan(_)) {
        ctx.write_authorized(authorization, spec).await?;
        return Ok(None);
    }

    let (signer, client, _) = ctx.signing_client_and_key(authorization).await?;
    let operation = client.execute_as(signer.clone(), spec.clone()).await?;
    ctx.report_tx(&operation);
    let checked = async {
        check_operation_status(&operation)?;
        let proposal_id = returned_proposal_id(&operation)?;
        let stored = ctx
            .client
            .read(dao::GetProposal {
                dao_id: spec.dao_id.clone(),
                id: proposal_id,
            })
            .await
            .context("read back the returned DAO proposal ID")?;
        ensure_proposal_matches(&stored.0, proposal_id, &signer.0, &spec.proposal)?;
        Ok::<_, anyhow::Error>(proposal_id)
    }
    .await;
    match checked {
        Ok(proposal_id) => Ok(Some(SubmittedProposal {
            proposal_id,
            proposal: spec.proposal,
            operation,
        })),
        Err(error) => {
            print_json(&operation)?;
            Err(error)
        }
    }
}

pub(crate) fn returned_proposal_id(operation: &WriteOperationResult) -> anyhow::Result<u64> {
    operation
        .operation
        .final_outcome()
        .context("add_proposal returned no final outcome")?
        .return_value_json()
        .context("add_proposal returned something other than an id")?
        .context("add_proposal returned no proposal id")
}

pub(crate) fn ensure_proposal_matches(
    stored: &dao::ProposalRecord,
    proposal_id: u64,
    proposer: &AccountId,
    expected: &ProposalInput,
) -> anyhow::Result<()> {
    anyhow::ensure!(
        stored.id == proposal_id,
        "DAO readback returned a different proposal ID"
    );
    anyhow::ensure!(
        stored.proposal.proposer == *proposer,
        "DAO proposal proposer does not match the submitting account"
    );
    anyhow::ensure!(
        stored.proposal.description == expected.description,
        "DAO proposal description does not match the submitted body"
    );
    anyhow::ensure!(
        stored.proposal.kind == expected.kind,
        "DAO proposal actions do not match the submitted body"
    );
    Ok(())
}
