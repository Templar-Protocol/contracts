//! DAO-governed MPC signing: propose a `sign` call, review what it signs, and
//! relay the signed payload once the proposal has executed.

use std::path::PathBuf;

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
    Broadcast, DeriveKey, ExecutedProposalArgs, InstallKey, Propose, Relay, ReviewArgs,
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
        Validity, TRANSACTION_VALIDITY_PERIOD_BLOCKS,
    },
    signer_contract::{
        DerivedPublicKeyArgs, KeyType, Payload, SignArgs, SignRequest, SignatureResponse,
    },
};

/// ≈7 days of mainnet blocks: our DAOs' default `proposal_period`.
const DEFAULT_VALID_FOR_BLOCKS: u64 = 1_000_000;
/// A public payload lives in the DAO's storage (≈1 NEAR per 100 KiB) and
/// inside one `add_proposal` transaction; contract code goes `--blind`.
const MAX_PUBLIC_DESCRIPTION_BYTES: usize = 64 * 1024;

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
    let valid_for_blocks = valid_for_blocks(args.kind, args.valid_for_blocks)?;
    let last_valid_block = block.height.saturating_add(valid_for_blocks);

    let payload = SignablePayload::build(BuildInputs {
        kind: args.kind,
        planned,
        public_key,
        nonce,
        block,
        valid_for_blocks,
    })?;
    let hash = payload.hash()?;
    let payload_hash = hex::encode(hash);
    let envelope = Envelope::new(payload);
    if let Some(out) = &args.out {
        envelope.write_file(out)?;
    }
    let description = proposal_description(&envelope, args.description, args.blind)?;

    let add_proposal = add_proposal_call(
        &dao,
        &mpc_contract_id,
        description,
        SignRequest {
            payload: Payload::for_hash(key_type, hash),
            path: path.clone(),
            domain_id: key_type.domain_id(),
        },
        args.sign_tgas,
        bond.proposal_bond,
    )?;

    tracing::info!(
        payload_hash,
        %public_key,
        nonce,
        last_valid_block,
        "built the payload the DAO will have signed"
    );
    if let Mode::Plan(_) = authorization.mode() {
        tracing::warn!(
            nonce,
            last_valid_block,
            "the printed plan carries a payload built now; it must be relayed by block {last_valid_block}"
        );
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

/// The operator's text, carrying the envelope unless the proposal is blind.
fn proposal_description(envelope: &Envelope, text: String, blind: bool) -> anyhow::Result<String> {
    if blind {
        Envelope::check_description(&text)?;
        return Ok(text);
    }
    let rendered = envelope.render_description(&text)?;
    anyhow::ensure!(
        rendered.len() <= MAX_PUBLIC_DESCRIPTION_BYTES,
        "the payload is {} bytes; the DAO would pay for that storage forever and a large one does \
         not fit an add_proposal transaction. Propose it with --blind --out",
        rendered.len()
    );
    Ok(rendered)
}

/// A delegate action's window is the operator's; a transaction's is the protocol's.
fn valid_for_blocks(kind: PayloadKind, requested: Option<u64>) -> anyhow::Result<u64> {
    match (kind, requested) {
        (PayloadKind::DelegateAction, requested) => {
            Ok(requested.unwrap_or(DEFAULT_VALID_FOR_BLOCKS))
        }
        (PayloadKind::Transaction, None) => Ok(TRANSACTION_VALIDITY_PERIOD_BLOCKS),
        (PayloadKind::Transaction, Some(_)) => anyhow::bail!(
            "--valid-for-blocks applies to a delegate action; a transaction is accepted for \
             {TRANSACTION_VALIDITY_PERIOD_BLOCKS} blocks after the block it is pinned to"
        ),
    }
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
    let next_nonce = full_access_next_nonce(ctx, controlled, public_key)
        .await?
        .with_context(|| {
            format!(
                "{controlled} has no full-access key {public_key}; install it with \
                 `tmplrmgr mpc install-key --dao {dao} --path {path} --signer-id {controlled} …`"
            )
        })?;
    Ok((public_key, next_nonce))
}

/// The next nonce of `public_key` on `account`, or `None` when the account
/// does not hold it as a full-access key. Any other failure is an error: a
/// flaky RPC must not read as a missing key.
async fn full_access_next_nonce(
    ctx: &CliContext,
    account: &AccountId,
    public_key: PublicKey,
) -> anyhow::Result<Option<u64>> {
    match ctx
        .client
        .read(account::GetAccessKey {
            account_id: account.clone(),
            public_key: public_key.into(),
        })
        .await
    {
        Ok(key) if key.permission == account::AccessKeyPermission::FullAccess => {
            Ok(Some(key.nonce + 1))
        }
        Ok(_) => Ok(None),
        Err(error) if is_unknown_access_key(&error) => Ok(None),
        Err(error) => Err(error).with_context(|| format!("read {account}'s key {public_key}")),
    }
}

/// The gateway erases the RPC's typed error, so the text is all that is left:
/// nearcore reports a missing key either as the `UnknownAccessKey` variant or,
/// in its legacy in-`result` form, as "access key … does not exist while
/// viewing" (a missing *account* reads "account … does not exist").
fn is_unknown_access_key(error: &templar_gateway_core::GatewayError) -> bool {
    let text = error.to_string();
    text.contains("UnknownAccessKey")
        || (text.contains("access key") && text.contains("does not exist while viewing"))
}

/// The `add_proposal` call that asks the DAO to have `request` signed.
fn add_proposal_call(
    dao: &AccountId,
    mpc_contract_id: &AccountId,
    description: String,
    request: SignRequest,
    sign_tgas: u64,
    bond: NearToken,
) -> anyhow::Result<tx::FunctionCall> {
    Ok(tx::FunctionCall {
        receiver_id: dao.clone(),
        method_name: ContractMethodName(dao::ADD_PROPOSAL_METHOD.to_owned()),
        args: ContractArgs::Json(serde_json::to_value(AddProposalArgs {
            proposal: ProposalInput {
                description,
                kind: sign_proposal_kind(dao, mpc_contract_id, request, sign_tgas)?,
            },
        })?),
        gas: dao::ADD_PROPOSAL_GAS,
        deposit: bond,
    })
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

pub(super) async fn show(ctx: CliContext, args: ReviewArgs) -> anyhow::Result<()> {
    print_json(&review(&ctx, &args).await?.output)
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
            args.executed.review.proposal_id
        ),
    }
}

pub(super) async fn broadcast(ctx: CliContext, args: Broadcast) -> anyhow::Result<()> {
    let signed_transaction = match signed_payload(&ctx, &args.executed).await? {
        Signed::Transaction(signed_transaction) => signed_transaction,
        Signed::DelegateAction(_) => anyhow::bail!(
            "proposal {} signed a delegate action, which needs a relayer; use `mpc relay`",
            args.executed.review.proposal_id
        ),
    };
    if args.print {
        return print_json(&SignedTransactionOutput {
            signed_transaction: Base64Bytes(borsh::to_vec(&signed_transaction)?),
        });
    }
    let tx_hash = signed_transaction.get_hash().into();
    let Some(outcome) = NearOperationExecutor::new(ctx.network_config().clone(), None)
        .submit_transaction(signed_transaction)
        .await?
    else {
        ctx.report_tx_hash(tx_hash);
        anyhow::bail!("transaction {tx_hash} was broadcast but its outcome is not yet known");
    };
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
    let review = review(ctx, &executed.review).await?;
    let output = &review.output;
    let proposal_id = executed.review.proposal_id;
    anyhow::ensure!(
        output.status == ProposalStatus::Approved,
        "proposal {proposal_id} is {:?}, not Approved",
        output.status
    );
    anyhow::ensure!(
        output.key_installed,
        "{} no longer holds the derived key {}; nothing can relay this payload",
        output.payload.signer_id,
        output.derived_public_key
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
    // One vote can execute several signing proposals; keep the one that verifies over this payload.
    let mut last_error = None;
    for receipt in &tx.receipts {
        if receipt.executor_id != output.mpc_contract_id {
            continue;
        }
        let Some(tx::ReturnValue::Json(value)) = &receipt.return_value else {
            continue;
        };
        match SignatureResponse::deserialize(value)
            .context("decode the MPC's response")
            .and_then(SignatureResponse::into_signature)
            .and_then(|signature| review.payload.sign(signature, output.derived_public_key))
        {
            Ok(signed) => return Ok(signed),
            Err(error) => last_error = Some(error),
        }
    }
    match last_error {
        None => anyhow::bail!(
            "transaction {} has no `sign` receipt from {} returning a signature; is it the \
             transaction that executed proposal {proposal_id}?",
            executed.tx_hash,
            output.mpc_contract_id,
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
    /// The derived key is a full-access key on the payload's signer right now.
    /// A historic proposal can be reviewed after the key was rotated out.
    key_installed: bool,
    /// The derived key has already signed at this nonce or a later one (or is
    /// gone): the payload was relayed, or superseded by another proposal.
    nonce_spent: bool,
}

struct Review {
    output: ReviewOutput,
    payload: SignablePayload,
}

async fn review(ctx: &CliContext, args: &ReviewArgs) -> anyhow::Result<Review> {
    let dao = &args.dao;
    let proposal_id = args.proposal_id;
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
    let expected_mpc_contract_id = args.mpc_contract_id(ctx.network());
    anyhow::ensure!(
        mpc_contract_id == expected_mpc_contract_id,
        "proposal {proposal_id} calls `sign` on {mpc_contract_id}, not the MPC contract \
         {expected_mpc_contract_id}; a look-alike could echo a public key it cannot sign for"
    );
    let key_type = request.key_type()?;
    let expected_hash = request.payload.hash()?;

    let envelope = match &args.payload_file {
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

    let (derived_public_key, last_valid_block, head_height) = tokio::try_join!(
        derived_public_key(ctx, &mpc_contract_id, key_type, dao, &request.path),
        last_valid_block(ctx, payload.validity),
        async { Ok(ctx.client.read(chain::GetBlock::default()).await?.height) },
    )?;
    anyhow::ensure!(
        payload.public_key == derived_public_key,
        "the payload is built for key {} but {dao} derives {derived_public_key} at path `{}`",
        payload.public_key,
        request.path
    );
    let key_next_nonce =
        full_access_next_nonce(ctx, &payload.signer_id, derived_public_key).await?;

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
            payload_file: args.payload_file.clone(),
            key_installed: key_next_nonce.is_some(),
            nonce_spent: key_next_nonce.is_none_or(|next| payload.nonce < next),
            payload,
            last_valid_block,
            head_height,
            // Anything sent now lands after `head_height`, so the last valid block is already too late.
            expired: head_height >= last_valid_block,
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
