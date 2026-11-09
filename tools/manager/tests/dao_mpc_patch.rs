use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use anyhow::{ensure, Context as _, Result};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use near_account_id::AccountId;
use serde_json::{json, Value};
use templar_gateway_methods_spec::{account, dao, market};
use templar_gateway_testing::{failed_receipts, SandboxHarness};
use templar_gateway_types::{ManagedAccountId, NearToken};

#[path = "common/market.rs"]
mod market_fixture;
use market_fixture::SeededMarket;

const BIN: &str = env!("CARGO_BIN_EXE_tmplrmgr");

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

async fn raw_storage(
    harness: &SandboxHarness,
    market: &AccountId,
) -> Result<BTreeMap<Vec<u8>, Vec<u8>>> {
    let client = templar_gateway_core::NearClient::with_finality_policy(
        harness.network.clone(),
        templar_gateway_core::FinalityPolicy::Final,
    );
    Ok(client
        .contract(market.clone())
        .state_with_prefix(Vec::new())
        .await?
        .into_iter()
        .map(|(key, value)| (key.0, value.0))
        .collect())
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
#[case::delegate("delegate-action", false, false)]
#[case::transaction("transaction", true, false)]
#[case::live_state_drift("delegate-action", false, true)]
#[tokio::test]
#[allow(
    clippy::too_many_lines,
    reason = "one real threshold-approved patch lifecycle binds CLI artifacts, signature receipts and raw storage across the chosen transport"
)]
async fn requires_sandbox_mpc_stamped_patch_executes_through_real_sputnik(
    #[case] transport: &str,
    #[case] raw: bool,
    #[case] drift: bool,
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
        harness
            .execute(
                &protocol,
                market::WithdrawStaticYield {
                    market_id: controlled.0.clone(),
                    amount: Some(1_u128.into()),
                },
            )
            .await?;
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
    let result = cli(harness, &execution, if raw { None } else { Some(&relayer) }).await?;
    ensure!(
        result.is_object(),
        "execution must emit structured transaction evidence"
    );
    harness.fast_forward(10).await?;
    let mut expected = before;
    expected.insert(b"STATE".to_vec(), expected_state);
    assert_eq!(raw_storage(harness, &controlled.0).await?, expected);
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
    Ok(())
}
