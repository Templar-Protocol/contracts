use std::path::{Path, PathBuf};

use anyhow::{ensure, Context as _, Result};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use near_account_id::AccountId;
use near_api::types::transaction::actions::{Action, TransferAction};
use serde_json::{json, Value};
use templar_gateway_core::PlannedTransaction;
use templar_gateway_methods_spec::{account, chain, dao, market, tx};
use templar_gateway_testing::{failed_receipts, SandboxHarness};
use templar_gateway_types::{common::TxExecutionStatus, ManagedAccountId, NearToken};

#[path = "common/market.rs"]
mod market_fixture;
use market_fixture::{finalize, SeededMarket};

#[path = "common/state.rs"]
mod state_fixture;
use state_fixture::raw_storage;

const BIN: &str = env!("CARGO_BIN_EXE_tmplrmgr");
const REQUEST_DESCRIPTION: &str = "asset-migration:test:patch:attempt-1";

struct RunDir(PathBuf);
impl RunDir {
    fn new() -> Result<Self> {
        let path = std::env::temp_dir().join(format!(
            "tmplrmgr-real-mpc-patch-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_nanos()
        ));
        std::fs::create_dir(&path)?;
        Ok(Self(path))
    }
}
impl Drop for RunDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

async fn invoke(
    harness: &SandboxHarness,
    args: &[&str],
    signer: Option<&ManagedAccountId>,
) -> Result<std::process::Output> {
    let mut command = tokio::process::Command::new(BIN);
    command.args([
        "--network",
        "testnet",
        "--rpc-url",
        harness.network.rpc_endpoints[0].url.as_str(),
        "-q",
    ]);
    command.args(args).env_remove("SECRET_KEY");
    if let Some(signer) = signer {
        command.args([
            "--signer-id",
            signer.as_str(),
            "--secret-key",
            &templar_gateway_testing::test_secret_key()?.to_string(),
        ]);
    }
    Ok(command.output().await?)
}

async fn succeeds(
    harness: &SandboxHarness,
    args: &[&str],
    signer: Option<&ManagedAccountId>,
) -> Result<std::process::Output> {
    let output = invoke(harness, args, signer).await?;
    ensure!(
        output.status.success(),
        "CLI failed for {args:?}: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(output)
}

async fn cli(
    harness: &SandboxHarness,
    args: &[&str],
    signer: Option<&ManagedAccountId>,
) -> Result<Value> {
    let output = succeeds(harness, args, signer).await?;
    serde_json::from_slice(&output.stdout)
        .with_context(|| format!("CLI stdout must be one structured JSON result for {args:?}"))
}

async fn proposal(harness: &SandboxHarness, dao_id: &AccountId, id: u64) -> Result<Value> {
    Ok(serde_json::to_value(
        harness
            .client()?
            .read(dao::GetProposal {
                dao_id: dao_id.clone(),
                id,
            })
            .await?,
    )?)
}

async fn approve(
    harness: &SandboxHarness,
    voter: &ManagedAccountId,
    dao_id: &AccountId,
    stored: &Value,
) -> Result<templar_gateway_types::common::WriteOperationResult> {
    harness
        .call_function(
            voter,
            dao_id,
            "act_proposal",
            json!({
                "id":stored["id"], "action":"VoteApprove", "proposal":stored["kind"], "memo":null
            }),
        )
        .await
}

fn text(path: &Path) -> Result<&str> {
    path.to_str().context("test artifact path must be UTF-8")
}

#[rstest::rstest]
#[case::delegate("delegate-action", false, false, false)]
#[case::transaction("transaction", true, false, false)]
#[case::live_state_drift("delegate-action", false, true, false)]
#[case::expired_transaction("transaction", true, false, true)]
#[tokio::test]
#[allow(
    clippy::too_many_lines,
    reason = "one real threshold-approved patch lifecycle binds CLI artifacts, signature receipts and raw storage across the chosen transport"
)]
async fn requires_sandbox_mpc_stamped_patch_executes_through_real_sputnik(
    #[case] transport: &str,
    #[case] raw: bool,
    #[case] drift: bool,
    #[case] expire: bool,
) -> Result<()> {
    let seeded = SeededMarket::new().await?;
    let harness = &seeded.harness;
    ensure!(
        seeded.observations["static_yield"]["amount"]
            .as_str()
            .context("static yield evidence")?
            .parse::<u128>()?
            > 0,
        "fixture must contain actual accrued static yield"
    );
    let proposer = harness.create_user("patch-proposer").await?;
    let council_a = harness.create_user("patch-council-a").await?;
    let council_b = harness.create_user("patch-council-b").await?;
    let outsider = harness.create_user("patch-outsider").await?;
    let relayer = harness.create_user("patch-relayer").await?;
    let mut policy = sputnikdao2::default_policy(vec![council_a.0.clone(), council_b.0.clone()]);
    policy.proposal_bond = NearToken::from_millinear(100);
    let dao_id = harness.deploy_sputnik_dao("patch-real-dao", policy).await?;
    let signer_id = harness.deploy_mock_signer("patch-local-signer").await?;
    let controlled = ManagedAccountId(seeded.market.market_id.clone());
    cli(
        harness,
        &[
            "dao",
            "mpc",
            "install-key",
            "--dao",
            dao_id.as_str(),
            "--mpc-contract",
            signer_id.as_str(),
        ],
        Some(&controlled),
    )
    .await?;
    let derived = cli(
        harness,
        &[
            "dao",
            "mpc",
            "derive-key",
            "--dao",
            dao_id.as_str(),
            "--mpc-contract",
            signer_id.as_str(),
            "--account-id",
            controlled.as_str(),
        ],
        None,
    )
    .await?;
    let derived_key = derived["public_key"]
        .as_str()
        .context("derived key result")?;
    let public_key = templar_gateway_types::primitive::PublicKey::from(
        derived_key.parse::<near_api::PublicKey>()?,
    );
    let run = RunDir::new()?;
    let changes = run.0.join("changes.json");
    let spec = run.0.join("patch.toml");
    let plan = run.0.join("patch-plan.json");
    let payload = run.0.join("payload.json");
    std::fs::write(
        &changes,
        serde_json::to_vec(&json!([
            {"op":"test","path":"/protocol_account_id","value":seeded.market.configuration.protocol_account_id},
            {"op":"replace","path":"/protocol_account_id","value":"approved-protocol.testnet"}
        ]))?,
    )?;
    cli(
        harness,
        &[
            "market",
            "generate-patch",
            "config",
            controlled.as_str(),
            "--changes",
            text(&changes)?,
            "--out",
            text(&spec)?,
        ],
        None,
    )
    .await?;
    succeeds(
        harness,
        &[
            "patch",
            "plan",
            text(&spec)?,
            "--out",
            text(&plan)?,
            "--signer-id",
            controlled.as_str(),
            "--public-key",
            derived_key,
        ],
        None,
    )
    .await?;
    succeeds(harness, &["patch", "dry-run", "--plan", text(&plan)?], None).await?;
    let reviewed: Value = serde_json::from_slice(&std::fs::read(&plan)?)?;
    ensure!(
        reviewed["dry_run"].is_object(),
        "actual sandbox replay must stamp the PatchPlan"
    );
    let before = raw_storage(harness, &controlled.0).await?;
    let original_code = harness
        .gateway_client()
        .contract(controlled.0.clone())
        .code()
        .await?;
    let expected_state = std::fs::read(run.0.join("patch.blobs/new-state.bin"))?;
    let submitted = cli(
        harness,
        &[
            "dao",
            "mpc",
            "propose",
            "--dao",
            dao_id.as_str(),
            "--mpc-contract",
            signer_id.as_str(),
            "--plan",
            text(&plan)?,
            "--kind",
            transport,
            "--description",
            REQUEST_DESCRIPTION,
            "--blind",
            "--out",
            text(&payload)?,
        ],
        Some(&proposer),
    )
    .await?;
    assert_eq!(submitted["source_plan_kind"], "patch_plan");
    let id = submitted["proposal_id"]
        .as_u64()
        .context("actual returned proposal ID")?;
    let stored = proposal(harness, &dao_id, id).await?;
    assert_eq!(stored["status"], "InProgress");
    let actions = stored["kind"]["FunctionCall"]["actions"]
        .as_array()
        .context("signature proposal actions")?;
    assert_eq!(actions.len(), 1);
    assert_eq!(
        stored["kind"]["FunctionCall"]["receiver_id"],
        signer_id.as_str()
    );
    assert_eq!(actions[0]["method_name"], "sign");
    let request: Value = serde_json::from_slice(
        &STANDARD.decode(actions[0]["args"].as_str().context("sign arguments")?)?,
    )?;
    assert_eq!(
        request["request"]["payload_v2"]["Eddsa"],
        submitted["payload_hash"]
    );
    let unauthorized = harness
        .try_call_function(
            &outsider,
            &dao_id,
            "act_proposal",
            json!({
                "id":id,"action":"VoteApprove","proposal":stored["kind"],"memo":null
            }),
        )
        .await?;
    assert!(failed_receipts(&unauthorized).any(|receipt| receipt.contract_id == dao_id));
    assert_eq!(
        proposal(harness, &dao_id, id).await?["status"],
        "InProgress"
    );
    approve(harness, &council_a, &dao_id, &stored).await?;
    assert_eq!(
        proposal(harness, &dao_id, id).await?["status"],
        "InProgress"
    );
    assert_eq!(raw_storage(harness, &controlled.0).await?, before);
    let vote = approve(harness, &council_b, &dao_id, &stored).await?;
    assert!(failed_receipts(&vote).next().is_none());
    let execution_hash = vote
        .operation
        .latest_tx_hash()
        .context("actual DAO execution transaction hash")?
        .to_string();
    assert_eq!(proposal(harness, &dao_id, id).await?["status"], "Approved");
    let id_text = id.to_string();
    let review_args = [
        "dao",
        "mpc",
        "show",
        "--dao",
        dao_id.as_str(),
        "--proposal-id",
        id_text.as_str(),
        "--mpc-contract",
        signer_id.as_str(),
        "--payload-file",
        text(&payload)?,
        "--plan",
        text(&plan)?,
    ];
    let mut described_review_args = review_args.to_vec();
    described_review_args.extend(["--expected-description", REQUEST_DESCRIPTION]);
    let mut execution = vec![
        "dao",
        "mpc",
        if raw { "broadcast" } else { "relay" },
        "--dao",
        dao_id.as_str(),
        "--proposal-id",
        id_text.as_str(),
        "--mpc-contract",
        signer_id.as_str(),
        "--payload-file",
        text(&payload)?,
        "--tx-hash",
        execution_hash.as_str(),
        "--tx-signer",
        council_b.as_str(),
        "--plan",
        text(&plan)?,
    ];
    // A modified source is rejected even though the DAO approved a genuine signature.
    let bad_plan = run.0.join("tampered-plan.json");
    let mut tampered = reviewed;
    tampered["batch"]["receiver_id"] = json!("other.testnet");
    std::fs::write(&bad_plan, serde_json::to_vec(&tampered)?)?;
    let last = execution.len() - 1;
    execution[last] = text(&bad_plan)?;
    let rejected = invoke(harness, &execution, if raw { None } else { Some(&relayer) }).await?;
    assert!(!rejected.status.success());
    assert_eq!(raw_storage(harness, &controlled.0).await?, before);
    execution[last] = text(&plan)?;
    if drift {
        let access_key = account::GetAccessKey {
            account_id: controlled.0.clone(),
            public_key: public_key.clone(),
        };
        let nonce = harness.client()?.read(access_key.clone()).await?.nonce;
        let protocol = ManagedAccountId(seeded.market.configuration.protocol_account_id.clone());
        let withdrawal = harness
            .execute(
                &protocol,
                market::WithdrawStaticYield {
                    market_id: controlled.0.clone(),
                    amount: Some(1_u128.into()),
                },
            )
            .await?;
        finalize(harness, &protocol.0, &withdrawal).await?;
        let drifted = raw_storage(harness, &controlled.0).await?;
        assert_ne!(drifted, before);
        let rejected = invoke(harness, &execution, if raw { None } else { Some(&relayer) }).await?;
        assert!(!rejected.status.success());
        assert_eq!(raw_storage(harness, &controlled.0).await?, drifted);
        assert_eq!(
            harness
                .gateway_client()
                .contract(controlled.0.clone())
                .code()
                .await?,
            original_code
        );
        assert_eq!(
            harness
                .get_configuration(&controlled.0)
                .await?
                .protocol_account_id,
            protocol.0
        );
        assert_eq!(harness.client()?.read(access_key).await?.nonce, nonce);
        return Ok(());
    }
    if expire {
        let client = harness.client()?;
        let period = client.read(chain::GetTransactionValidityPeriod).await?;
        let access_key = account::GetAccessKey {
            account_id: controlled.0.clone(),
            public_key: public_key.clone(),
        };
        let nonce = client.read(access_key.clone()).await?.nonce;
        // Explicitly cross the real chain's bound, independent of block cadence.
        harness.fast_forward(period.saturating_add(1)).await?;
        let expired_review = cli(harness, &described_review_args, None).await?;
        assert_eq!(expired_review["expired"], true);
        assert_eq!(
            expired_review["source_plan_digest"],
            submitted["source_plan_digest"]
        );
        let rejected = invoke(harness, &execution, None).await?;
        assert!(!rejected.status.success());
        assert!(
            rejected.stdout.is_empty(),
            "no submission identity should be emitted"
        );
        assert_eq!(client.read(access_key).await?.nonce, nonce);
        assert_eq!(raw_storage(harness, &controlled.0).await?, before);
        assert_eq!(
            harness
                .gateway_client()
                .contract(controlled.0.clone())
                .code()
                .await?,
            original_code
        );
        return Ok(());
    }
    let executed = cli(harness, &execution, if raw { None } else { Some(&relayer) }).await?;
    // Submission waits for Executed, but raw_storage reads Final. An optimistic
    // height jump does not finalize the delegated patch's receipt block.
    let finalized = harness
        .client()?
        .read(tx::Get {
            tx_hash: serde_json::from_value(executed["tx_hash"].clone())?,
            sender_account_id: serde_json::from_value(executed["sender_account_id"].clone())?,
            wait_until: Some(TxExecutionStatus::Final),
            encoding: tx::ValueEncoding::Json,
        })
        .await?;
    ensure!(
        finalized.status == tx::Status::Succeeded && finalized.failed_receipts.is_empty(),
        "approved patch must finalize successfully with no failed child receipts: {finalized:?}"
    );
    let mut expected = before;
    expected.insert(b"STATE".to_vec(), expected_state);
    assert_eq!(
        raw_storage(harness, &controlled.0).await?,
        expected,
        "patch execution result: {executed}"
    );
    assert_eq!(
        harness
            .gateway_client()
            .contract(controlled.0.clone())
            .code()
            .await?,
        original_code
    );
    assert_eq!(
        harness
            .get_configuration(&controlled.0)
            .await?
            .protocol_account_id
            .as_str(),
        "approved-protocol.testnet"
    );
    let nonce = harness
        .client()?
        .read(account::GetAccessKey {
            account_id: controlled.0.clone(),
            public_key: public_key.clone(),
        })
        .await?
        .nonce;
    // Review remains possible after the patch changed live storage and consumed
    // the signed nonce. It validates the frozen source, not pre-execution state.
    let terminal_review = cli(harness, &described_review_args, None).await?;
    assert_eq!(terminal_review["status"], "Approved");
    assert_eq!(terminal_review["nonce_spent"], true);
    assert_eq!(terminal_review["source_plan_kind"], "patch_plan");
    assert_eq!(
        terminal_review["source_plan_digest"],
        submitted["source_plan_digest"]
    );
    assert_eq!(terminal_review["description"], REQUEST_DESCRIPTION);
    let replay = invoke(harness, &execution, if raw { None } else { Some(&relayer) }).await?;
    assert!(!replay.status.success());
    assert_eq!(
        harness
            .client()?
            .read(account::GetAccessKey {
                account_id: controlled.0.clone(),
                public_key
            })
            .await?
            .nonce,
        nonce
    );
    assert_eq!(raw_storage(harness, &controlled.0).await?, expected);
    if !drift {
        // Independent review rejection cases run only after execution, so they
        // cannot consume the original raw transaction's fixed validity window.
        // An unsealed local envelope can be replaced with another genuinely
        // approved request at the same key/path. Approval alone is not binding
        // to the frozen migration plan.
        let mut wrong_description = review_args.to_vec();
        wrong_description.extend([
            "--expected-description",
            "asset-migration:test:patch:attempt",
        ]);
        let rejected = invoke(harness, &wrong_description, None).await?;
        assert!(!rejected.status.success());
        let other_plan = run.0.join("other-plan.json");
        let other_payload = run.0.join("other-payload.json");
        std::fs::write(
            &other_plan,
            serde_json::to_vec(&PlannedTransaction::single_action(
                controlled.clone(),
                outsider.0.clone(),
                Action::Transfer(TransferAction {
                    deposit: NearToken::from_yoctonear(1),
                }),
            ))?,
        )?;
        let other = cli(
            harness,
            &[
                "dao",
                "mpc",
                "propose",
                "--dao",
                dao_id.as_str(),
                "--mpc-contract",
                signer_id.as_str(),
                "--plan",
                text(&other_plan)?,
                "--kind",
                transport,
                "--description",
                "different request",
                "--out",
                text(&other_payload)?,
            ],
            Some(&proposer),
        )
        .await?;
        assert_eq!(other["path"], submitted["path"]);
        assert_eq!(other["derived_public_key"], submitted["derived_public_key"]);
        assert_eq!(other["nonce"].as_u64(), Some(nonce + 1));
        assert_ne!(other["payload_hash"], submitted["payload_hash"]);
        let other_id = other["proposal_id"].as_u64().context("other proposal ID")?;
        let other_stored = proposal(harness, &dao_id, other_id).await?;
        approve(harness, &council_a, &dao_id, &other_stored).await?;
        let other_vote = approve(harness, &council_b, &dao_id, &other_stored).await?;
        assert!(failed_receipts(&other_vote).next().is_none());
        assert_eq!(
            proposal(harness, &dao_id, other_id).await?["status"],
            "Approved"
        );
        let other_id_text = other_id.to_string();
        let mut recovery_review = vec![
            "dao",
            "mpc",
            "show",
            "--dao",
            dao_id.as_str(),
            "--proposal-id",
            other_id_text.as_str(),
            "--mpc-contract",
            signer_id.as_str(),
            "--payload-file",
            text(&other_payload)?,
        ];
        let planless = cli(harness, &recovery_review, None).await?;
        assert_eq!(planless["status"], "Approved");
        assert_eq!(planless["description"], other_stored["description"]);
        assert!(planless.get("source_plan_digest").is_none());
        recovery_review.extend(["--plan", text(&plan)?]);
        let rejected = invoke(harness, &recovery_review, None).await?;
        assert!(!rejected.status.success());
        *recovery_review
            .last_mut()
            .context("recovery plan argument")? = text(&other_plan)?;
        let other_review = cli(harness, &recovery_review, None).await?;
        assert_eq!(
            other_review["source_plan_digest"],
            other["source_plan_digest"]
        );
        assert_eq!(other_review["description"], other_stored["description"]);
        recovery_review.extend(["--expected-description", REQUEST_DESCRIPTION]);
        let rejected = invoke(harness, &recovery_review, None).await?;
        assert!(!rejected.status.success());
        *recovery_review
            .last_mut()
            .context("expected description argument")? = "different request";
        let described_other = cli(harness, &recovery_review, None).await?;
        assert_eq!(described_other["description"], other_stored["description"]);
        assert_eq!(
            described_other["source_plan_digest"],
            other["source_plan_digest"]
        );
        let mut wrong_source = review_args;
        *wrong_source.last_mut().context("review plan argument")? = text(&other_plan)?;
        let rejected = invoke(harness, &wrong_source, None).await?;
        assert!(!rejected.status.success());
        assert_eq!(raw_storage(harness, &controlled.0).await?, expected);
    }
    Ok(())
}
