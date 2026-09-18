//! DAO-governed MPC signing: propose a `sign` call, review what it signs, and
//! relay the signed payload once the proposal has executed.

use std::path::{Path, PathBuf};

use anyhow::Context as _;
use near_account_id::AccountId;
use near_api::{
    types::transaction::actions::{Action, FunctionCallAction},
    PublicKey,
};
use serde::{Deserialize, Serialize};
use sputnikdao2::{ProposalInput, ProposalStatus};
use templar_gateway_core::{ExecuteOperation as _, NearOperationExecutor, PlannedTransaction};
use templar_gateway_methods_spec::{account, chain, tx};
use templar_gateway_types::{
    common::{ContractArgs, TxExecutionStatus, WriteOperationResult},
    Base64Bytes, ContractMethodName, ManagedAccountId, NearGas, NearToken,
};

use crate::commands::mpc::{
    Broadcast, DeriveKey, ExecutedProposalArgs, InstallKey, Propose, Relay, Show,
};
use crate::commands::signer::{Authorization, Mode};
use crate::context::{print_json, sputnik_function_call, CliContext};
use crate::mpc::{
    dao::{
        self, AddProposalArgs, GetPolicyArgs, GetProposalArgs, PolicyBond, ProposalView,
        ProposedSignature,
    },
    envelope::Envelope,
    payload::{
        transaction_last_valid_block, BuildInputs, Decoded, PayloadKind, SignablePayload, Signed,
        Validity,
    },
    signer_contract::{
        DerivedPublicKeyArgs, KeyType, Payload, SignArgs, SignRequest, SignatureResponse,
    },
};

#[derive(Serialize)]
struct DerivedKeyOutput {
    mpc_contract_id: AccountId,
    dao: AccountId,
    path: String,
    key_type: KeyType,
    public_key: PublicKey,
}

pub(super) async fn derive_key(ctx: CliContext, args: DeriveKey) -> anyhow::Result<()> {
    let mpc_contract_id = args.mpc.contract_id(ctx.network());
    let path = args.derivation.path_for(&args.account_id);
    let public_key = derived_public_key(
        &ctx,
        &mpc_contract_id,
        args.mpc.key_type,
        &args.derivation.dao,
        &path,
    )
    .await?;
    print_json(&DerivedKeyOutput {
        mpc_contract_id,
        dao: args.derivation.dao,
        path,
        key_type: args.mpc.key_type,
        public_key,
    })
}

pub(super) async fn install_key(ctx: CliContext, args: InstallKey) -> anyhow::Result<()> {
    let controlled = args.signer.account_id();
    let public_key = derived_public_key(
        &ctx,
        &args.mpc.contract_id(ctx.network()),
        args.mpc.key_type,
        &args.derivation.dao,
        &args.derivation.path_for(&controlled),
    )
    .await?;
    tracing::info!(%public_key, "adding the derived key with full access");
    ctx.write(
        args.signer,
        account::AddKey {
            public_key: public_key.into(),
            permission: account::AccessKeyPermission::FullAccess,
        },
    )
    .await
}

#[derive(Serialize)]
struct ProposeOutput {
    dao: AccountId,
    proposal_id: u64,
    mpc_contract_id: AccountId,
    path: String,
    key_type: KeyType,
    derived_public_key: PublicKey,
    kind: PayloadKind,
    nonce: u64,
    payload_hash: String,
    last_valid_block: u64,
    blind: bool,
    payload_file: Option<PathBuf>,
    add_proposal: WriteOperationResult,
}

pub(super) async fn propose(ctx: CliContext, args: Propose) -> anyhow::Result<()> {
    let authorization = Authorization::try_from(&args.signer)?;
    let planned: PlannedTransaction = crate::commands::load_json_file(&args.plan)?;
    let controlled = planned.signer_account_id.0.clone();
    let dao = args.derivation.dao.clone();
    let mpc_contract_id = args.mpc.contract_id(ctx.network());
    let key_type = args.mpc.key_type;
    let path = args.derivation.path_for(&controlled);

    let ((public_key, next_nonce), block, bond) = tokio::try_join!(
        installed_derived_key(&ctx, &mpc_contract_id, key_type, &dao, &path, &controlled),
        async { Ok(ctx.client.read(chain::GetBlock::default()).await?) },
        ctx.view::<PolicyBond>(&dao, dao::GET_POLICY_METHOD, &GetPolicyArgs {}),
    )?;
    let nonce = args.nonce.unwrap_or(next_nonce);
    anyhow::ensure!(
        nonce >= next_nonce,
        "--nonce {nonce} is below the key's next nonce {next_nonce}; the chain would reject the \
         signed result after the DAO has voted on it"
    );
    let last_valid_block = match args.kind {
        PayloadKind::DelegateAction => block.height.saturating_add(args.valid_for_blocks),
        PayloadKind::Transaction => transaction_last_valid_block(block.height),
    };

    let payload = SignablePayload::build(BuildInputs {
        kind: args.kind,
        planned,
        public_key,
        nonce,
        block,
        valid_for_blocks: args.valid_for_blocks,
    })?;
    let hash = payload.hash()?;
    let payload_hash = hex::encode(hash);
    let envelope = Envelope::new(payload);
    if let Some(out) = &args.out {
        envelope.write_file(out)?;
    }
    let description = if args.blind {
        args.description
    } else {
        envelope.render_description(&args.description)?
    };

    let add_proposal = tx::FunctionCall {
        receiver_id: dao.clone(),
        method_name: ContractMethodName(dao::ADD_PROPOSAL_METHOD.to_owned()),
        args: ContractArgs::Json(serde_json::to_value(AddProposalArgs {
            proposal: ProposalInput {
                description,
                kind: sign_proposal_kind(
                    &dao,
                    &mpc_contract_id,
                    SignRequest {
                        payload: Payload::for_hash(key_type, hash),
                        path: path.clone(),
                        domain_id: key_type.domain_id(),
                    },
                    args.sign_tgas,
                )?,
            },
        })?),
        gas: dao::ADD_PROPOSAL_GAS,
        deposit: bond.proposal_bond,
    };

    tracing::info!(
        payload_hash,
        %public_key,
        nonce,
        last_valid_block,
        "built the payload the DAO will have signed"
    );
    if let Mode::Plan(_) = authorization.mode() {
        return ctx.write_authorized(authorization, add_proposal).await;
    }

    let (signer, client, _) = ctx.signing_client_and_key(authorization).await?;
    let result = client.execute_as(signer, add_proposal).await?;
    ctx.report_checked(&result)?;
    let proposal_id = result
        .operation
        .final_outcome()
        .and_then(|outcome| outcome.return_value_json().transpose())
        .context("add_proposal returned no proposal id")?
        .context("add_proposal returned something other than an id")?;
    tracing::info!(proposal_id, "created proposal");

    print_json(&ProposeOutput {
        dao,
        proposal_id,
        mpc_contract_id,
        path,
        key_type,
        derived_public_key: public_key,
        kind: args.kind,
        nonce,
        payload_hash,
        last_valid_block,
        blind: args.blind,
        payload_file: args.out,
        add_proposal: result,
    })
}

/// The derived key, which must already be a full-access key on `controlled`,
/// and the nonce its next transaction should carry.
async fn installed_derived_key(
    ctx: &CliContext,
    mpc_contract_id: &AccountId,
    key_type: KeyType,
    dao: &AccountId,
    path: &str,
    controlled: &AccountId,
) -> anyhow::Result<(PublicKey, u64)> {
    let public_key = derived_public_key(ctx, mpc_contract_id, key_type, dao, path).await?;
    let access_key = ctx
        .client
        .read(account::GetAccessKey {
            account_id: controlled.clone(),
            public_key: public_key.into(),
        })
        .await
        .with_context(|| {
            format!(
                "{controlled} has no access key {public_key}; install it with \
                 `tmplrmgr mpc install-key --dao {dao} --path {path} --signer-id {controlled} …`"
            )
        })?;
    anyhow::ensure!(
        access_key.permission == account::AccessKeyPermission::FullAccess,
        "{public_key} on {controlled} is a function-call key; MPC signing needs full access"
    );
    Ok((public_key, access_key.nonce + 1))
}

/// The proposal kind Sputnik executes: the DAO calling `sign` on the MPC contract.
fn sign_proposal_kind(
    dao: &AccountId,
    mpc_contract_id: &AccountId,
    request: SignRequest,
    sign_tgas: u64,
) -> anyhow::Result<sputnikdao2::ProposalKind> {
    sputnik_function_call(PlannedTransaction::single_action(
        ManagedAccountId(dao.clone()),
        mpc_contract_id.clone(),
        Action::FunctionCall(Box::new(FunctionCallAction {
            method_name: dao::SIGN_METHOD.to_owned(),
            args: serde_json::to_vec(&SignArgs { request })?,
            gas: NearGas::from_tgas(sign_tgas),
            deposit: dao::SIGN_DEPOSIT,
        })),
    ))
}

pub(super) async fn show(ctx: CliContext, args: Show) -> anyhow::Result<()> {
    let review = review(
        &ctx,
        &args.dao,
        args.proposal_id,
        args.payload_file.as_deref(),
    )
    .await?;
    print_json(&review.output)
}

pub(super) async fn relay(ctx: CliContext, args: Relay) -> anyhow::Result<()> {
    match signed_payload(&ctx, &args.executed).await? {
        Signed::DelegateAction(signed_delegate_action) => {
            ctx.write(
                args.signer,
                tx::RelaySignedDelegateAction {
                    signed_delegate_action,
                },
            )
            .await
        }
        Signed::Transaction(_) => anyhow::bail!(
            "proposal {} signed a transaction, which needs no relayer; use `mpc broadcast`",
            args.executed.proposal_id
        ),
    }
}

pub(super) async fn broadcast(ctx: CliContext, args: Broadcast) -> anyhow::Result<()> {
    let signed_transaction = match signed_payload(&ctx, &args.executed).await? {
        Signed::Transaction(signed_transaction) => signed_transaction,
        Signed::DelegateAction(_) => anyhow::bail!(
            "proposal {} signed a delegate action, which needs a relayer; use `mpc relay`",
            args.executed.proposal_id
        ),
    };
    if args.print {
        return print_json(&SignedTransactionOutput {
            signed_transaction: Base64Bytes(borsh::to_vec(&signed_transaction)?),
        });
    }
    let outcome = NearOperationExecutor::new(ctx.network_config().clone(), None)
        .submit_transaction(signed_transaction)
        .await?
        .context("the transaction was broadcast but its outcome is not yet known")?;
    ctx.report_tx_hash(outcome.tx_hash);
    let succeeded = outcome.is_success;
    print_json(&BroadcastOutput {
        tx_hash: outcome.tx_hash,
        outcome: outcome.outcome,
    })?;
    anyhow::ensure!(succeeded, "the broadcast transaction failed on chain");
    Ok(())
}

/// The reviewed payload with the signature its executed proposal produced.
async fn signed_payload(
    ctx: &CliContext,
    executed: &ExecutedProposalArgs,
) -> anyhow::Result<Signed> {
    let review = review(
        ctx,
        &executed.dao,
        executed.proposal_id,
        executed.payload_file.as_deref(),
    )
    .await?;
    let output = &review.output;
    anyhow::ensure!(
        output.status == ProposalStatus::Approved,
        "proposal {} is {:?}, not Approved",
        executed.proposal_id,
        output.status
    );
    anyhow::ensure!(
        !output.expired,
        "the payload expired at block {}; head is {}. Propose it again",
        output.last_valid_block,
        output.head_height
    );
    anyhow::ensure!(
        !output.nonce_spent,
        "the derived key has already signed at nonce {} or later: this payload was relayed, or \
         superseded by a newer proposal",
        output.payload.nonce
    );

    let tx = ctx
        .client
        .read(tx::Get {
            tx_hash: executed.tx_hash.into(),
            sender_account_id: executed.tx_signer.clone(),
            wait_until: Some(TxExecutionStatus::Final),
            encoding: tx::ValueEncoding::Json,
        })
        .await?;
    // One vote can execute several signing proposals; the signature that
    // verifies over this payload's hash is the one that belongs to it.
    let mut last_error = None;
    for receipt in &tx.receipts {
        if receipt.executor_id != output.mpc_contract_id {
            continue;
        }
        let Some(tx::ReturnValue::Json(value)) = &receipt.return_value else {
            continue;
        };
        let Ok(response) = SignatureResponse::deserialize(value) else {
            continue;
        };
        match response
            .into_signature()
            .and_then(|signature| review.payload.sign(signature, output.derived_public_key))
        {
            Ok(signed) => return Ok(signed),
            Err(error) => last_error = Some(error),
        }
    }
    match last_error {
        None => anyhow::bail!(
            "transaction {} has no `sign` receipt from {} returning a signature; is it the \
             transaction that executed proposal {}?",
            executed.tx_hash,
            output.mpc_contract_id,
            executed.proposal_id
        ),
        Some(error) => {
            Err(error.context("no signature in the transaction verifies over this payload"))
        }
    }
}

/// What `near transaction send-signed-transaction` accepts.
#[derive(Serialize)]
struct SignedTransactionOutput {
    signed_transaction: Base64Bytes,
}

#[derive(Serialize)]
struct BroadcastOutput {
    tx_hash: templar_gateway_types::CryptoHash,
    outcome: templar_gateway_types::operation::ExecutionOutcome,
}

/// Everything a voter needs to judge a proposal, all cross-checked: the payload
/// hashes to what the DAO will have signed, and names the key the DAO derives.
#[derive(Serialize)]
struct ReviewOutput {
    dao: AccountId,
    proposal_id: u64,
    status: ProposalStatus,
    proposer: AccountId,
    description: String,
    mpc_contract_id: AccountId,
    path: String,
    key_type: KeyType,
    derived_public_key: PublicKey,
    /// What the DAO attaches to the `sign` call.
    sign_deposit: NearToken,
    sign_gas: NearGas,
    payload_hash: String,
    /// Where the payload was read from; `None` means the proposal description.
    payload_file: Option<PathBuf>,
    payload: Decoded,
    /// From the payload bytes: a delegate action's own bound, or the validity
    /// window after the on-chain height of the block a transaction is pinned to.
    last_valid_block: u64,
    head_height: u64,
    expired: bool,
    /// The derived key has already signed at this nonce or a later one: the
    /// payload was relayed, or superseded by another proposal.
    nonce_spent: bool,
}

struct Review {
    output: ReviewOutput,
    payload: SignablePayload,
}

async fn review(
    ctx: &CliContext,
    dao: &AccountId,
    proposal_id: u64,
    payload_file: Option<&Path>,
) -> anyhow::Result<Review> {
    let proposal: ProposalView = ctx
        .view(
            dao,
            dao::GET_PROPOSAL_METHOD,
            &GetProposalArgs { id: proposal_id },
        )
        .await?;
    let ProposedSignature {
        mpc_contract_id,
        request,
        deposit,
        gas,
    } = proposal.proposed_signature()?;
    let key_type = request.key_type()?;
    let expected_hash = request.payload.hash()?;

    let envelope = match payload_file {
        Some(path) => Envelope::read_file(path)?,
        None => Envelope::from_description(&proposal.description)?.with_context(|| {
            format!(
                "proposal {proposal_id} carries no payload in its description (a blind \
                 proposal?); pass --payload-file"
            )
        })?,
    };
    let hash = envelope.payload.hash()?;
    anyhow::ensure!(
        hash == expected_hash,
        "the payload hashes to {} but proposal {proposal_id} asks the MPC to sign {}",
        hex::encode(hash),
        hex::encode(expected_hash)
    );
    let payload = envelope.payload.decode()?;

    let ((derived_public_key, key_next_nonce), last_valid_block, head_height) = tokio::try_join!(
        installed_derived_key(
            ctx,
            &mpc_contract_id,
            key_type,
            dao,
            &request.path,
            &payload.signer_id,
        ),
        last_valid_block(ctx, payload.validity),
        async { Ok(ctx.client.read(chain::GetBlock::default()).await?.height) },
    )?;
    anyhow::ensure!(
        payload.public_key == derived_public_key,
        "the payload is built for key {} but {dao} derives {derived_public_key} at path `{}`",
        payload.public_key,
        request.path
    );

    Ok(Review {
        output: ReviewOutput {
            dao: dao.clone(),
            proposal_id,
            status: proposal.status,
            proposer: proposal.proposer,
            description: proposal.description,
            mpc_contract_id,
            path: request.path,
            key_type,
            derived_public_key,
            sign_deposit: deposit,
            sign_gas: gas,
            payload_hash: hex::encode(hash),
            payload_file: payload_file.map(Path::to_path_buf),
            nonce_spent: payload.nonce < key_next_nonce,
            payload,
            last_valid_block,
            head_height,
            expired: head_height > last_valid_block,
        },
        payload: envelope.payload,
    })
}

async fn last_valid_block(ctx: &CliContext, validity: Validity) -> anyhow::Result<u64> {
    match validity {
        Validity::MaxBlockHeight(height) => Ok(height),
        Validity::BlockHash(block_hash) => {
            let block = ctx
                .client
                .read(chain::GetBlock {
                    block_hash: Some(block_hash.into()),
                })
                .await
                .with_context(|| {
                    format!(
                        "the transaction is pinned to block {block_hash}, which this RPC no \
                         longer serves: it has most likely expired; an archival RPC can confirm"
                    )
                })?;
            Ok(transaction_last_valid_block(block.height))
        }
    }
}

async fn derived_public_key(
    ctx: &CliContext,
    mpc_contract_id: &AccountId,
    key_type: KeyType,
    dao: &AccountId,
    path: &str,
) -> anyhow::Result<PublicKey> {
    ctx.view(
        mpc_contract_id,
        "derived_public_key",
        &DerivedPublicKeyArgs {
            path: path.to_owned(),
            predecessor: dao.clone(),
            domain_id: key_type.domain_id(),
        },
    )
    .await
}
