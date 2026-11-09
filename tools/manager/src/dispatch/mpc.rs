//! DAO-governed MPC signing: propose a `sign` call, review what it signs, and
//! relay the signed payload once the proposal has executed.

use std::path::PathBuf;

use anyhow::Context as _;
use near_account_id::AccountId;
use near_api::types::Signature;
use near_api::{
    types::transaction::{
        actions::{Action, FunctionCallAction},
        delegate_action::{DelegateAction, NonDelegateAction, SignedDelegateAction},
        SignedTransaction, Transaction, TransactionV0,
    },
    PublicKey,
};
use serde::{Deserialize, Serialize};
use sputnikdao2::{ProposalInput, ProposalStatus};
use templar_gateway_core::{ExecuteOperation as _, NearOperationExecutor, PlannedTransaction};
use templar_gateway_methods_spec::{account, chain, dao as dao_methods, tx};
use templar_gateway_types::{
    common::{TxExecutionStatus, WriteOperationResult},
    Base64Bytes, ManagedAccountId, NearGas, NearToken,
};

use crate::commands::dao::mpc::{
    Broadcast, DeriveKey, ExecutedProposalArgs, InstallKey, Propose, Relay, ReviewArgs,
};
use crate::commands::signer::Authorization;
use crate::context::{print_json, sputnik_function_call, CliContext};
use crate::mpc::{
    dao::{self, ProposedSignature},
    envelope::Envelope,
    payload::{
        transaction_last_valid_block, BuildInputs, Decoded, PayloadKind, SignablePayload, Signed,
        Validity, TRANSACTION_VALIDITY_PERIOD_BLOCKS,
    },
    signer_contract::{
        DerivedPublicKeyArgs, KeyType, Payload, SignArgs, SignRequest, SignatureResponse,
    },
    source_plan::{SourcePlan, SourcePlanKind},
};

/// ≈7 days of mainnet blocks: our DAOs' default `proposal_period`.
const DEFAULT_VALID_FOR_BLOCKS: u64 = 1_000_000;
/// nearcore's `AccessKey::ACCESS_KEY_NONCE_RANGE_MULTIPLIER`: a nonce must stay
/// below `(block_height + 1) × this`.
const NONCE_RANGE_PER_BLOCK: u64 = 1_000_000;
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
    let path = args.path();
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
    source_plan_kind: SourcePlanKind,
    source_plan_digest: crate::spec::plan::WireSha256Digest,
    add_proposal: WriteOperationResult,
}

pub(super) async fn propose(ctx: CliContext, args: Propose) -> anyhow::Result<()> {
    let authorization = Authorization::try_from(&args.signer)?;
    let source = SourcePlan::load(&args.plan)?;
    let controlled = source.transaction().signer_account_id.0.clone();
    let dao = args.derivation.dao.clone();
    let mpc_contract_id = args.mpc.contract_id(ctx.network());
    let key_type = args.mpc.key_type;
    let path = args.derivation.path_for(&controlled);

    let ((public_key, next_nonce), block) = tokio::try_join!(
        installed_derived_key(&ctx, &mpc_contract_id, key_type, &dao, &path, &controlled),
        async { Ok(ctx.client.read(chain::GetBlock::default()).await?) },
    )?;
    let nonce = args.nonce.unwrap_or(next_nonce);
    anyhow::ensure!(
        nonce >= next_nonce,
        "--nonce {nonce} is below the key's next nonce {next_nonce}; the chain would reject the \
         signed result after the DAO has voted on it"
    );
    let nonce_ceiling = (block.height + 1) * NONCE_RANGE_PER_BLOCK;
    anyhow::ensure!(
        nonce < nonce_ceiling,
        "--nonce {nonce} is at or above the chain's ceiling {nonce_ceiling} for block {}; the \
         chain would reject the signed result as NonceTooLarge",
        block.height
    );
    let valid_for_blocks = valid_for_blocks(args.kind, args.valid_for_blocks)?;
    let last_valid_block = block.height.saturating_add(valid_for_blocks);

    let payload = SignablePayload::build(BuildInputs {
        kind: args.kind,
        planned: source.transaction().clone(),
        public_key,
        nonce,
        block,
        valid_for_blocks,
    })?;
    source.ensure_matches(&payload.decode()?)?;
    validate_transport(&ctx, &payload).await?;
    let hash = payload.hash()?;
    let payload_hash = hex::encode(hash);
    let envelope = Envelope::new(payload);
    let description = proposal_description(&envelope, args.description, args.blind)?;
    if let Some(out) = &args.out {
        envelope.write_file(out)?;
    }

    let proposal = ProposalInput {
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
    };
    tracing::info!(
        payload_hash,
        %public_key,
        nonce,
        last_valid_block,
        "built the payload the DAO will have signed"
    );
    validate_source_live(&ctx, &source, public_key).await?;
    let Some(submitted) =
        super::dao::submit_proposal(&ctx, authorization, dao.clone(), proposal).await?
    else {
        return Ok(());
    };

    print_json(&ProposeOutput {
        dao,
        proposal_id: submitted.proposal_id,
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
        source_plan_kind: source.kind(),
        source_plan_digest: source.digest(),
        add_proposal: submitted.operation,
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
                "{controlled} has no access key {public_key}; install it with \
                 `tmplrmgr dao mpc install-key --dao {dao} --path {path} --signer-id {controlled} …`"
            )
        })?;
    Ok((public_key, next_nonce))
}

/// The next nonce of `public_key` on `account`, or `None` when the account
/// does not hold it. A downgraded key and a failed RPC are errors: neither
/// must read as a missing key.
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
        Ok(_) => anyhow::bail!(
            "{public_key} on {account} is a function-call key; MPC signing needs full access"
        ),
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

async fn validate_source_live(
    ctx: &CliContext,
    source: &SourcePlan,
    public_key: PublicKey,
) -> anyhow::Result<()> {
    if let Some(plan) = source.patch() {
        super::patch::validate_live_plan(
            ctx,
            plan,
            &source.transaction().signer_account_id.0,
            public_key,
            super::patch::LivePlanOptions::default(),
        )
        .await?;
    }
    Ok(())
}

fn placeholder_signature(public_key: PublicKey) -> anyhow::Result<Signature> {
    let len = match public_key {
        PublicKey::ED25519(_) => 64,
        PublicKey::SECP256K1(_) => 65,
    };
    Signature::from_parts(public_key.key_type(), &[0; 65][..len])
        .context("construct transport size signature")
}

/// Freeze the chosen transport before the DAO accepts a bond. A relay is sized
/// with the largest legal account/key/signature because its fee payer is not
/// known until execution. The actual signed transaction is checked again.
fn transport_for_bounds(payload: &SignablePayload) -> anyhow::Result<SignedTransaction> {
    let decoded = payload.decode()?;
    let transaction = match decoded.kind {
        PayloadKind::Transaction => TransactionV0 {
            signer_id: decoded.signer_id,
            public_key: decoded.public_key,
            nonce: decoded.nonce,
            receiver_id: decoded.receiver_id,
            block_hash: match decoded.validity {
                Validity::BlockHash(hash) => hash,
                Validity::MaxBlockHeight(_) => anyhow::bail!("transaction has no block hash"),
            },
            actions: decoded.actions,
        },
        PayloadKind::DelegateAction => {
            let delegate = SignedDelegateAction {
                signature: placeholder_signature(decoded.public_key)?,
                delegate_action: DelegateAction {
                    sender_id: decoded.signer_id.clone(),
                    receiver_id: decoded.receiver_id,
                    public_key: decoded.public_key,
                    nonce: decoded.nonce,
                    max_block_height: match decoded.validity {
                        Validity::MaxBlockHeight(height) => height,
                        Validity::BlockHash(_) => anyhow::bail!("delegate has no expiry height"),
                    },
                    actions: decoded
                        .actions
                        .into_iter()
                        .map(|action| {
                            NonDelegateAction::try_from(action)
                                .map_err(|()| anyhow::anyhow!("nested delegate action"))
                        })
                        .collect::<anyhow::Result<_>>()?,
                },
            };
            TransactionV0 {
                signer_id: "a".repeat(64).parse()?,
                public_key: PublicKey::empty(near_api::types::crypto::KeyType::SECP256K1),
                nonce: u64::MAX,
                receiver_id: decoded.signer_id,
                block_hash: near_api::types::CryptoHash([0; 32]),
                actions: vec![Action::Delegate(Box::new(delegate))],
            }
        }
    };
    Ok(SignedTransaction::new(
        placeholder_signature(transaction.public_key)?,
        Transaction::V0(transaction),
    ))
}

async fn validate_transport(ctx: &CliContext, payload: &SignablePayload) -> anyhow::Result<()> {
    validate_signed_transport(ctx, &transport_for_bounds(payload)?).await
}

// nearcore validates the recursive FunctionCall prepaid sum against this
// limit. Action execution/send fees are charged separately, not added to it.
fn prepaid_gas(action: &Action) -> anyhow::Result<u64> {
    match action {
        Action::FunctionCall(call) => Ok(call.gas.as_gas()),
        Action::Delegate(delegate) => {
            delegate
                .delegate_action
                .actions
                .iter()
                .try_fold(0_u64, |total, action| {
                    total
                        .checked_add(prepaid_gas(action)?)
                        .context("sum delegate prepaid gas")
                })
        }
        _ => Ok(0),
    }
}

fn ensure_transport_limits(
    transaction: &SignedTransaction,
    limits: &templar_gateway_types::ProtocolLimits,
) -> anyhow::Result<()> {
    let size = borsh::to_vec(transaction)?.len();
    anyhow::ensure!(
        size as u64 <= limits.max_transaction_size,
        "chosen MPC transport requires {size} signed bytes; live limit is {}",
        limits.max_transaction_size
    );
    let gas = transaction
        .transaction
        .actions()
        .iter()
        .try_fold(0_u64, |total, action| {
            total
                .checked_add(prepaid_gas(action)?)
                .context("sum transport prepaid gas")
        })?;
    anyhow::ensure!(
        gas <= limits.max_total_prepaid_gas.as_gas(),
        "chosen MPC transport requires {gas} prepaid gas; live limit is {}",
        limits.max_total_prepaid_gas.as_gas()
    );
    Ok(())
}

async fn validate_signed_transport(
    ctx: &CliContext,
    transaction: &SignedTransaction,
) -> anyhow::Result<()> {
    let limits = ctx.client.read(chain::GetProtocolLimits).await?;
    ensure_transport_limits(transaction, &limits)
}

pub(super) async fn relay(ctx: CliContext, args: Relay) -> anyhow::Result<()> {
    let source = SourcePlan::load(&args.plan)?;
    let delegate = match signed_payload(&ctx, &args.executed, &source).await? {
        Signed::DelegateAction(delegate) => delegate,
        Signed::Transaction(_) => anyhow::bail!(
            "proposal {} signed a transaction, which needs no relayer; use `dao mpc broadcast`",
            args.executed.review.proposal_id
        ),
    };
    let delegate = delegate.into_inner();
    let public_key = delegate.delegate_action.public_key;
    let authorization = Authorization::try_from(&args.signer)?;
    if matches!(authorization.mode(), crate::commands::signer::Mode::Plan(_)) {
        validate_source_live(&ctx, &source, public_key).await?;
        return ctx
            .write_authorized(
                authorization,
                tx::RelaySignedDelegateAction {
                    signed_delegate_action: delegate.into(),
                },
            )
            .await;
    }
    let (signer, _) = authorization.resolve(ctx.network_config()).await?;
    let lease = signer.lease_next().await;
    let transaction = lease
        .presign(
            ctx.network_config(),
            delegate.delegate_action.sender_id.clone(),
            vec![Action::Delegate(Box::new(delegate))],
        )
        .await?;
    validate_signed_transport(&ctx, &transaction).await?;
    validate_source_live(&ctx, &source, public_key).await?;
    submit_signed_transport(&ctx, transaction, &source).await
}

pub(super) async fn broadcast(ctx: CliContext, args: Broadcast) -> anyhow::Result<()> {
    let source = SourcePlan::load(&args.plan)?;
    let signed_transaction = match signed_payload(&ctx, &args.executed, &source).await? {
        Signed::Transaction(signed_transaction) => signed_transaction,
        Signed::DelegateAction(_) => anyhow::bail!(
            "proposal {} signed a delegate action, which needs a relayer; use `dao mpc relay`",
            args.executed.review.proposal_id
        ),
    };
    validate_signed_transport(&ctx, &signed_transaction).await?;
    validate_source_live(&ctx, &source, signed_transaction.transaction.public_key()).await?;
    if args.print {
        return print_json(&SignedTransactionOutput {
            signed_transaction: Base64Bytes(borsh::to_vec(&signed_transaction)?),
        });
    }
    submit_signed_transport(&ctx, signed_transaction, &source).await
}

/// The signed hash is known before RPC submission, even if the response is lost.
async fn submit_signed_transport(
    ctx: &CliContext,
    transaction: SignedTransaction,
    source: &SourcePlan,
) -> anyhow::Result<()> {
    let tx_hash = transaction.get_hash().into();
    let sender_account_id = transaction.transaction.signer_id().clone();
    let result = NearOperationExecutor::new(ctx.network_config().clone(), None)
        .submit_transaction(transaction)
        .await;
    let (status, outcome) = match &result {
        Ok(Some(outcome))
            if outcome.is_success
                && outcome.outcome.receipts.iter().all(|receipt| {
                    receipt.status != templar_gateway_types::operation::ReceiptStatus::Failed
                }) =>
        {
            ("succeeded", Some(&outcome.outcome))
        }
        Ok(Some(outcome)) => ("failed", Some(&outcome.outcome)),
        Ok(None) | Err(_) => ("unknown", None),
    };
    ctx.report_tx_hash(tx_hash);
    print_json(&BroadcastOutput {
        tx_hash,
        sender_account_id,
        status,
        source_plan_kind: source.kind(),
        source_plan_digest: source.digest(),
        outcome,
    })?;
    let outcome = result.context("signed transaction submission failed; reconcile the printed transaction identity before any retry")?
        .context("transaction was broadcast but its outcome is not yet known")?;
    anyhow::ensure!(
        outcome.is_success,
        "the broadcast transaction failed on chain"
    );
    let failed: Vec<_> = outcome
        .outcome
        .receipts
        .iter()
        .filter(|receipt| receipt.status == templar_gateway_types::operation::ReceiptStatus::Failed)
        .map(|receipt| receipt.contract_id.as_str())
        .collect();
    anyhow::ensure!(
        failed.is_empty(),
        "the transaction contains failed receipts on: {}",
        failed.join(", ")
    );
    Ok(())
}

/// The reviewed payload with the signature its executed proposal produced.
async fn signed_payload(
    ctx: &CliContext,
    executed: &ExecutedProposalArgs,
    source: &SourcePlan,
) -> anyhow::Result<Signed> {
    let review = review(ctx, &executed.review).await?;
    let output = &review.output;
    source.ensure_matches(&output.payload)?;
    validate_transport(ctx, &review.payload).await?;
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
        if receipt.executor_id != output.mpc_contract_id
            || receipt.status != templar_gateway_types::operation::ReceiptStatus::Succeeded
        {
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
struct BroadcastOutput<'a> {
    tx_hash: templar_gateway_types::CryptoHash,
    sender_account_id: AccountId,
    status: &'static str,
    source_plan_kind: SourcePlanKind,
    source_plan_digest: crate::spec::plan::WireSha256Digest,
    outcome: Option<&'a templar_gateway_types::operation::ExecutionOutcome>,
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
    let proposal = ctx
        .client
        .read(dao_methods::GetProposal {
            dao_id: dao.clone(),
            id: proposal_id,
        })
        .await?
        .0;
    let ProposedSignature {
        mpc_contract_id,
        request,
        deposit,
        gas,
    } = dao::proposed_signature(&proposal)?;
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
        None => Envelope::from_description(&proposal.proposal.description)?.with_context(|| {
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
            status: proposal.proposal.status,
            proposer: proposal.proposal.proposer,
            description: proposal.proposal.description,
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

#[cfg(test)]
mod tests {
    use super::*;
    use near_api::types::transaction::actions::DeployContractAction;

    #[rstest::rstest]
    #[case(PayloadKind::DelegateAction)]
    #[case(PayloadKind::Transaction)]
    fn chosen_transport_checks_exact_signed_size_and_nested_prepaid_gas(#[case] kind: PayloadKind) {
        let key = near_api::signer::generate_secret_key().expect("test key");
        let actions = vec![
            Action::DeployContract(DeployContractAction {
                code: vec![1; 1_024],
            }),
            Action::FunctionCall(Box::new(FunctionCallAction {
                method_name: "patch".to_owned(),
                args: vec![7; 256],
                gas: NearGas::from_tgas(300),
                deposit: NearToken::from_yoctonear(0),
            })),
            Action::DeployContract(DeployContractAction {
                code: vec![2; 2_048],
            }),
        ];
        let payload = SignablePayload::build(BuildInputs {
            kind,
            planned: PlannedTransaction {
                signer_account_id: ManagedAccountId("market.near".parse().unwrap()),
                receiver_id: "market.near".parse().unwrap(),
                actions: actions.clone(),
                continue_on_failure: false,
            },
            public_key: key.public_key(),
            nonce: 10,
            block: templar_gateway_types::BlockSummary {
                height: 1_000,
                timestamp_ns: 0,
                gas_price: NearToken::from_yoctonear(1),
                hash: near_api::types::CryptoHash([1; 32]).into(),
            },
            valid_for_blocks: 100,
        })
        .unwrap();
        let signed = transport_for_bounds(&payload).unwrap();
        let size = borsh::to_vec(&signed).unwrap().len() as u64;
        let limits = templar_gateway_types::ProtocolLimits {
            max_transaction_size: size,
            max_total_prepaid_gas: NearGas::from_tgas(300),
            max_length_storage_key: 1_024,
            max_length_storage_value: 4_194_304,
            num_bytes_account: 100,
            num_extra_bytes_record: 40,
        };
        ensure_transport_limits(&signed, &limits).unwrap();
        assert!(ensure_transport_limits(
            &signed,
            &templar_gateway_types::ProtocolLimits {
                max_transaction_size: size - 1,
                ..limits
            },
        )
        .is_err());
        assert!(ensure_transport_limits(
            &signed,
            &templar_gateway_types::ProtocolLimits {
                max_total_prepaid_gas: NearGas::from_gas(300_000_000_000_000 - 1),
                ..limits
            },
        )
        .is_err());
        match signed.transaction.actions() {
            [Action::Delegate(delegate)] => {
                assert_eq!(delegate.delegate_action.actions.len(), 3);
                for (actual, expected) in delegate.delegate_action.actions.iter().zip(&actions) {
                    assert_eq!(
                        borsh::to_vec(actual).unwrap(),
                        borsh::to_vec(expected).unwrap()
                    );
                }
                assert_eq!(signed.transaction.signer_id().as_str().len(), 64);
                assert!(matches!(
                    signed.transaction.public_key(),
                    PublicKey::SECP256K1(_)
                ));
            }
            actual => assert_eq!(actual, &actions),
        }
    }
}
