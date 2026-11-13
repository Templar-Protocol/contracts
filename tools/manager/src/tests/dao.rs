use clap::{error::ErrorKind, Parser};
use near_account_id::AccountId;
use near_api::types::transaction::actions::{Action, FunctionCallAction, TransferAction};
use serde_json::json;
use templar_gateway_core::PlannedTransaction;
use templar_gateway_methods_spec::dao;
use templar_gateway_types::{Base64Bytes, ManagedAccountId, NearGas, NearToken};

use super::CREDS;
use crate::cli::{Cli, Command};
use crate::commands::dao::DaoNs;
use crate::dispatch::dao::{ensure_proposal_matches, ordinary_proposal};

fn plan() -> PlannedTransaction {
    PlannedTransaction {
        signer_account_id: ManagedAccountId::from(
            "dao.testnet".parse::<AccountId>().expect("account"),
        ),
        receiver_id: "receiver.testnet".parse().expect("account"),
        actions: vec![
            Action::FunctionCall(Box::new(FunctionCallAction {
                method_name: "first".to_owned(),
                args: vec![0, 255, 1],
                gas: NearGas::from_tgas(17),
                deposit: NearToken::from_yoctonear(5),
            })),
            Action::FunctionCall(Box::new(FunctionCallAction {
                method_name: "second".to_owned(),
                args: b" {\"ordered\": true} ".to_vec(),
                gas: NearGas::from_tgas(23),
                deposit: NearToken::from_yoctonear(7),
            })),
        ],
        continue_on_failure: false,
    }
}

#[test]
fn dao_commands_parse_without_legacy_mpc_alias() {
    let error = Cli::try_parse_from(["tmplrmgr", "mpc"])
        .expect_err("the old top-level namespace must not remain");
    assert_eq!(error.kind(), ErrorKind::InvalidSubcommand);
    let cli = Cli::try_parse_from(
        [
            "tmplrmgr",
            "dao",
            "propose",
            "--dao",
            "dao.testnet",
            "--plan",
            "-",
            "--description",
            "ordered calls",
        ]
        .into_iter()
        .chain(CREDS),
    )
    .expect("ordinary proposal parses");
    let Command::Dao {
        command: DaoNs::Propose(args),
    } = cli.command
    else {
        panic!("expected ordinary DAO proposal");
    };
    assert_eq!(args.dao.as_str(), "dao.testnet");
    assert_eq!(args.plan, std::path::Path::new("-"));
    assert_eq!(args.description, "ordered calls");
    assert!(matches!(
        Cli::try_parse_from([
            "tmplrmgr", "dao", "get-proposal", "--dao", "dao.testnet", "--proposal-id", "42",
        ])
        .expect("reading needs no signer")
        .command,
        Command::Dao {
            command: DaoNs::GetProposal(args)
        } if args.proposal_id == 42
    ));
}

#[test]
fn ordinary_proposal_requires_its_input_and_description() {
    for missing in ["--plan", "--description"] {
        let mut args = vec!["tmplrmgr", "dao", "propose", "--dao", "dao.testnet"];
        if missing != "--plan" {
            args.extend(["--plan", "plan.json"]);
        }
        if missing != "--description" {
            args.extend(["--description", "review"]);
        }
        let error = Cli::try_parse_from(args.into_iter().chain(CREDS))
            .expect_err("required ordinary proposal argument");
        assert_eq!(error.kind(), ErrorKind::MissingRequiredArgument);
    }
}

#[test]
fn ordinary_proposal_preserves_exact_function_call_content() -> anyhow::Result<()> {
    let original = plan();
    let proposal = ordinary_proposal(
        &original.signer_account_id.0,
        original.clone(),
        "review".into(),
    )?;
    let body = serde_json::to_value(proposal)?;
    assert_eq!(body["description"], "review");
    assert_eq!(
        body["kind"]["FunctionCall"]["receiver_id"],
        "receiver.testnet"
    );
    let actions = body["kind"]["FunctionCall"]["actions"]
        .as_array()
        .expect("actions");
    assert_eq!(actions.len(), original.actions.len());
    for (actual, expected) in actions.iter().zip(original.actions) {
        let Action::FunctionCall(expected) = expected else {
            unreachable!("test fixture contains only calls");
        };
        assert_eq!(actual["method_name"], expected.method_name);
        assert_eq!(
            actual["args"],
            serde_json::to_value(Base64Bytes(expected.args))?
        );
        assert_eq!(actual["gas"], serde_json::to_value(expected.gas)?);
        assert_eq!(actual["deposit"], serde_json::to_value(expected.deposit)?);
    }
    Ok(())
}

#[test]
fn ordinary_proposal_rejects_foreign_signer_empty_non_call_and_failure_policy() {
    let dao = "dao.testnet".parse().expect("account");
    let mut foreign = plan();
    foreign.signer_account_id =
        ManagedAccountId::from("other.testnet".parse::<AccountId>().expect("account"));
    let mut empty = plan();
    empty.actions.clear();
    let mut transfer = plan();
    transfer.actions.push(Action::Transfer(TransferAction {
        deposit: NearToken::from_yoctonear(1),
    }));
    let mut continuation = plan();
    continuation.continue_on_failure = true;
    for invalid in [foreign, empty, transfer, continuation] {
        assert!(ordinary_proposal(&dao, invalid, "review".into()).is_err());
    }
}

#[test]
fn proposal_readback_requires_exact_id_proposer_description_and_actions() -> anyhow::Result<()> {
    let proposer: AccountId = "proposer.testnet".parse()?;
    let input = ordinary_proposal(&"dao.testnet".parse()?, plan(), "review".into())?;
    let body = serde_json::to_value(&input)?;
    let stored = json!({
        "id": 42, "proposer": proposer, "description": body["description"],
        "kind": body["kind"], "status": "InProgress", "vote_counts": {},
        "votes": {}, "submission_time": "1", "last_actions_log": []
    });
    let decoded: dao::ProposalOutput = serde_json::from_value(stored.clone())?;
    ensure_proposal_matches(&decoded.0, 42, &proposer, &input)?;
    assert!(ensure_proposal_matches(&decoded.0, 41, &proposer, &input).is_err());
    assert!(ensure_proposal_matches(&decoded.0, 42, &"other.testnet".parse()?, &input).is_err());
    for (pointer, replacement) in [
        ("/description", json!("different")),
        ("/kind/FunctionCall/receiver_id", json!("other.testnet")),
        ("/kind/FunctionCall/actions/0/args", json!("AQ==")),
        ("/kind/FunctionCall/actions/0/gas", json!("1")),
        ("/kind/FunctionCall/actions/0/deposit", json!("1")),
    ] {
        let mut changed = stored.clone();
        *changed.pointer_mut(pointer).expect("existing field") = replacement;
        let changed: dao::ProposalOutput = serde_json::from_value(changed)?;
        assert!(ensure_proposal_matches(&changed.0, 42, &proposer, &input).is_err());
    }
    let mut reordered = stored;
    reordered["kind"]["FunctionCall"]["actions"]
        .as_array_mut()
        .expect("actions")
        .reverse();
    let reordered: dao::ProposalOutput = serde_json::from_value(reordered)?;
    assert!(ensure_proposal_matches(&reordered.0, 42, &proposer, &input).is_err());
    Ok(())
}

async fn run(
    harness: &templar_gateway_testing::SandboxHarness,
    args: &[&str],
) -> anyhow::Result<()> {
    let rpc = harness.network.rpc_endpoints[0].url.to_string();
    let cli = Cli::try_parse_from(
        ["tmplrmgr", "-q", "--rpc-url", &rpc]
            .into_iter()
            .chain(args.iter().copied()),
    )?;
    crate::dispatch::dispatch(crate::context::build_context(&cli)?, cli.command).await
}

#[allow(
    clippy::too_many_lines,
    reason = "one real Sputnik lifecycle covers CLI authoring, exact readback and threshold execution"
)]
#[tokio::test]
async fn requires_sandbox_dao_ordinary_proposal_executes_through_real_threshold(
) -> anyhow::Result<()> {
    use templar_gateway_testing::{failed_receipts, SandboxHarness};

    let harness = SandboxHarness::start_owned().await?;
    let proposer = harness.create_user("ordinary-proposer").await?;
    let council_a = harness.create_user("ordinary-council-a").await?;
    let council_b = harness.create_user("ordinary-council-b").await?;
    let outsider = harness.create_user("ordinary-outsider").await?;
    let policy: dao::Policy = serde_json::from_value(json!({
        "roles": [
            {"name": "proposers", "kind": {"Group": [proposer]},
             "permissions": ["call:AddProposal"], "vote_policy": {}},
            {"name": "council", "kind": {"Group": [council_a, council_b]},
             "permissions": ["*:VoteApprove"], "vote_policy": {}}
        ],
        "default_vote_policy": {"weight_kind": "RoleWeight", "quorum": "2", "threshold": "2"},
        "proposal_bond": NearToken::from_millinear(100),
        "proposal_period": "604800000000000", "bounty_bond": "0",
        "bounty_forgiveness_period": "86400000000000"
    }))?;
    let dao_id = harness.deploy_sputnik_dao("ordinary-dao", policy.0).await?;
    let secret = templar_gateway_testing::test_secret_key()?.to_string();
    let mut transaction = PlannedTransaction::single_action(
        ManagedAccountId::from(dao_id.clone()),
        harness.ft_contract_id.clone(),
        Action::FunctionCall(Box::new(FunctionCallAction {
            method_name: "set_redemption_rate".to_owned(),
            args: serde_json::to_vec(&json!({"redemption_rate": "42"}))?,
            gas: NearGas::from_tgas(30),
            deposit: NearToken::ZERO,
        })),
    );
    let path = std::env::temp_dir().join(format!("tmplrmgr-dao-{dao_id}.json"));
    std::fs::write(&path, serde_json::to_vec(&transaction)?)?;
    let common = [
        "dao",
        "propose",
        "--dao",
        dao_id.as_str(),
        "--plan",
        path.to_str().expect("UTF-8 fixture path"),
        "--description",
        "threshold call",
    ];
    assert!(run(
        &harness,
        &[
            &common[..],
            &["--signer-id", outsider.as_str(), "--secret-key", &secret]
        ]
        .concat(),
    )
    .await
    .is_err());
    run(
        &harness,
        &[
            &common[..],
            &["--signer-id", proposer.as_str(), "--secret-key", &secret],
        ]
        .concat(),
    )
    .await?;
    let client = harness.client()?;
    let stored = client
        .read(dao::GetProposal {
            dao_id: dao_id.clone(),
            id: 0,
        })
        .await?;
    let expected = ordinary_proposal(&dao_id, transaction.clone(), "threshold call".into())?;
    ensure_proposal_matches(&stored.0, 0, &proposer.0, &expected)?;
    assert_eq!(
        stored.0.proposal.status,
        sputnikdao2::ProposalStatus::InProgress
    );
    run(
        &harness,
        &[
            "dao",
            "get-proposal",
            "--dao",
            dao_id.as_str(),
            "--proposal-id",
            "0",
        ],
    )
    .await?;
    let kind = serde_json::to_value(&stored.0.proposal.kind)?;
    for (index, voter) in [&council_a, &council_b].into_iter().enumerate() {
        let voted = harness
            .try_call_function(
                voter,
                &dao_id,
                "act_proposal",
                json!({
                    "id": 0, "action": "VoteApprove", "proposal": kind, "memo": null
                }),
            )
            .await?;
        assert!(failed_receipts(&voted).next().is_none());
        let status = client
            .read(dao::GetProposal {
                dao_id: dao_id.clone(),
                id: 0,
            })
            .await?
            .0
            .proposal
            .status;
        assert_eq!(
            status,
            if index == 0 {
                sputnikdao2::ProposalStatus::InProgress
            } else {
                sputnikdao2::ProposalStatus::Approved
            }
        );
    }
    assert_eq!(
        harness
            .view_json::<String>(&harness.ft_contract_id, "redemption_rate", json!({}))
            .await?,
        "42"
    );
    // A second submission must validate the new returned ID, not re-read proposal zero.
    transaction.actions[0] = Action::FunctionCall(Box::new(FunctionCallAction {
        method_name: "set_redemption_rate".to_owned(),
        args: serde_json::to_vec(&json!({"redemption_rate": "43"}))?,
        gas: NearGas::from_tgas(31),
        deposit: NearToken::ZERO,
    }));
    std::fs::write(&path, serde_json::to_vec(&transaction)?)?;
    run(
        &harness,
        &[
            &common[..],
            &["--signer-id", proposer.as_str(), "--secret-key", &secret],
        ]
        .concat(),
    )
    .await?;
    let second = client
        .read(dao::GetProposal {
            dao_id: dao_id.clone(),
            id: 1,
        })
        .await?;
    ensure_proposal_matches(
        &second.0,
        1,
        &proposer.0,
        &ordinary_proposal(&dao_id, transaction, "threshold call".into())?,
    )?;
    assert_ne!(second.0.proposal.kind, stored.0.proposal.kind);
    std::fs::remove_file(path)?;
    Ok(())
}
