use clap::{error::ErrorKind, Parser};

use super::CREDS;
use crate::cli::{Cli, Command};
use crate::commands::account::AccountNs;
use crate::commands::mpc::MpcNs;
use crate::mpc::payload::PayloadKind;
use templar_gateway_methods_spec::account as spec;

const PROPOSE: [&str; 8] = [
    "tmplrmgr",
    "mpc",
    "propose",
    "--dao",
    "dao.testnet",
    "--plan",
    "plan.json",
    "--kind",
];

#[test]
fn propose_defaults_to_a_delegate_action_and_the_conventional_path() {
    let cli = Cli::try_parse_from(
        [
            "tmplrmgr",
            "mpc",
            "propose",
            "--dao",
            "dao.testnet",
            "--plan",
            "-",
        ]
        .into_iter()
        .chain(CREDS),
    )
    .expect("propose should parse");
    let Command::Mpc {
        command: MpcNs::Propose(propose),
    } = cli.command
    else {
        panic!("expected Mpc::Propose")
    };
    assert_eq!(propose.kind, PayloadKind::DelegateAction);
    assert!(!propose.blind);
    assert_eq!(
        propose
            .derivation
            .path_for(&"target.testnet".parse().expect("valid")),
        "dao.testnet-target.testnet"
    );
    assert_eq!(
        propose
            .mpc
            .contract_id(templar_gateway_client::Network::Testnet)
            .as_str(),
        "v1.signer-prod.testnet"
    );
}

#[test]
fn propose_accepts_the_transaction_kind() {
    let cli = Cli::try_parse_from(PROPOSE.into_iter().chain(["transaction"]).chain(CREDS))
        .expect("propose should parse");
    let Command::Mpc {
        command: MpcNs::Propose(propose),
    } = cli.command
    else {
        panic!("expected Mpc::Propose")
    };
    assert_eq!(propose.kind, PayloadKind::Transaction);
}

/// A blind proposal's bytes exist nowhere but the file, so refusing to write
/// one is refusing to make the proposal relayable.
#[test]
fn blind_requires_out() {
    let error = Cli::try_parse_from(
        PROPOSE
            .into_iter()
            .chain(["delegate-action", "--blind"])
            .chain(CREDS),
    )
    .expect_err("--blind without --out");
    assert_eq!(error.kind(), ErrorKind::MissingRequiredArgument);

    Cli::try_parse_from(
        PROPOSE
            .into_iter()
            .chain(["delegate-action", "--blind", "--out", "payload.json"])
            .chain(CREDS),
    )
    .expect("--blind with --out parses");
}

const RELAY: [&str; 11] = [
    "tmplrmgr",
    "mpc",
    "relay",
    "--dao",
    "dao.testnet",
    "--proposal-id",
    "7",
    "--tx-hash",
    "11111111111111111111111111111111",
    "--tx-signer",
    "voter.testnet",
];

#[test]
fn relay_takes_the_relayer_credentials() {
    let Command::Mpc {
        command: MpcNs::Relay(relay),
    } = Cli::try_parse_from(RELAY.into_iter().chain(CREDS))
        .expect("relay parses")
        .command
    else {
        panic!("expected Mpc::Relay")
    };
    assert_eq!(relay.proposal_id, 7);
    assert_eq!(relay.tx_signer.as_str(), "voter.testnet");
    assert!(Cli::try_parse_from(RELAY).is_err());
}

#[test]
fn add_key_takes_a_literal_key_or_a_derivation_but_needs_one() {
    let base = ["tmplrmgr", "account", "add-key"];

    let error = Cli::try_parse_from(base.into_iter().chain(CREDS)).expect_err("no key source");
    assert_eq!(error.kind(), ErrorKind::MissingRequiredArgument);

    let Command::Account {
        command: AccountNs::AddKey(add_key),
    } = Cli::try_parse_from(
        base.into_iter()
            .chain(["--mpc-dao", "dao.testnet"])
            .chain(CREDS),
    )
    .expect("derivation parses")
    .command
    else {
        panic!("expected Account::AddKey")
    };
    let derivation = add_key.derivation().expect("a derivation was requested");
    assert_eq!(derivation.dao.as_str(), "dao.testnet");
    assert_eq!(derivation.path, format!("dao.testnet-{}", CREDS[1]));
    assert!(add_key.literal_public_key().is_none());

    let error = Cli::try_parse_from(
        base.into_iter()
            .chain(["--mpc-path", "custom"])
            .chain(CREDS),
    )
    .expect_err("--mpc-path alone");
    assert_eq!(error.kind(), ErrorKind::MissingRequiredArgument);
}

const KEY: &str = "ed25519:6phW8vfVNMmktunyZV576gGomMurYvg4ZkQHMdbGSiXd";

#[test]
fn add_key_defaults_to_full_access_and_restricts_with_receiver_id() {
    let base = ["tmplrmgr", "account", "add-key", "--key", KEY];
    let parse = |extra: &[&str]| {
        let Command::Account {
            command: AccountNs::AddKey(add_key),
        } = Cli::try_parse_from(base.into_iter().chain(extra.iter().copied()).chain(CREDS))
            .expect("parses")
            .command
        else {
            panic!("expected Account::AddKey")
        };
        add_key.permission()
    };

    assert_eq!(parse(&[]), spec::AccessKeyPermission::FullAccess);
    assert_eq!(
        parse(&[
            "--receiver-id",
            "market.testnet",
            "--method-name",
            "borrow",
            "--method-name",
            "repay",
            "--allowance",
            "1 NEAR",
        ]),
        spec::AccessKeyPermission::FunctionCall {
            allowance: Some(templar_gateway_types::NearToken::from_near(1)),
            receiver_id: "market.testnet".parse().expect("valid"),
            method_names: vec![
                templar_gateway_types::ContractMethodName::from("borrow".to_owned()),
                templar_gateway_types::ContractMethodName::from("repay".to_owned()),
            ],
        }
    );

    let error = Cli::try_parse_from(
        base.into_iter()
            .chain(["--method-name", "borrow"])
            .chain(CREDS),
    )
    .expect_err("--method-name without --receiver-id");
    assert_eq!(error.kind(), ErrorKind::MissingRequiredArgument);
}

#[test]
fn delete_key_parses() {
    let Command::Account {
        command: AccountNs::DeleteKey(delete_key),
    } = Cli::try_parse_from(
        ["tmplrmgr", "account", "delete-key", "--key", KEY]
            .into_iter()
            .chain(CREDS),
    )
    .expect("parses")
    .command
    else {
        panic!("expected Account::DeleteKey")
    };
    assert_eq!(delete_key.into_spec().public_key.0.to_string(), KEY);
}

/// The whole operator flow against a mock MPC signer and a mock DAO: install
/// the derived key, propose, have the "MPC" sign the hash the proposal carries,
/// approve, review, relay — and the controlled account's transfer lands.
#[rstest::rstest]
#[case::public_delegate_action(PayloadKind::DelegateAction, false)]
#[case::blind_transaction(PayloadKind::Transaction, true)]
#[tokio::test]
async fn requires_sandbox_mpc_proposal_is_relayed_end_to_end(
    #[case] kind: PayloadKind,
    #[case] blind: bool,
) -> anyhow::Result<()> {
    use near_api::types::transaction::actions::{Action, TransferAction};
    use templar_gateway_core::PlannedTransaction;
    use templar_gateway_methods_spec::{account, contract, tx};
    use templar_gateway_types::{
        common::ContractArgs, operation::ReceiptStatus, ContractMethodName, NearGas, NearToken,
    };

    use crate::mpc::{
        envelope::Envelope,
        signer_contract::{DerivedPublicKeyArgs, KeyType},
    };

    let harness = templar_gateway_testing::SandboxHarness::start_owned().await?;
    let rpc_url = harness.network.rpc_endpoints[0].url.to_string();
    let secret_key = templar_gateway_testing::test_secret_key()?.to_string();
    let controlled = harness.create_user("mpc-target").await?;
    let proposer = harness.create_user("mpc-proposer").await?;
    let relayer = harness.create_user("mpc-relayer").await?;
    let beneficiary = harness.create_user("mpc-beneficiary").await?;
    let bond = NearToken::from_millinear(100);
    let signer_id = harness.deploy_mock_signer("mpc-signer").await?;
    let dao = harness.deploy_mock_dao("mpc-dao", bond).await?;
    let client = harness.client()?;

    let mpc_key = near_api::signer::generate_secret_key()?;
    let path = format!("{dao}-{}", *controlled);
    client
        .execute_as(
            proposer.clone(),
            tx::FunctionCall {
                receiver_id: signer_id.clone(),
                method_name: ContractMethodName("set_derived_public_key".to_owned()),
                args: ContractArgs::Json(serde_json::to_value(SetDerivedPublicKey {
                    derivation: DerivedPublicKeyArgs {
                        path: path.clone(),
                        predecessor: dao.clone(),
                        domain_id: KeyType::Ed25519.domain_id(),
                    },
                    public_key: mpc_key.public_key().to_string(),
                })?),
                gas: NearGas::from_tgas(30),
                deposit: NearToken::ZERO,
            },
        )
        .await?;

    let run = |args: Vec<String>| {
        let rpc_url = rpc_url.clone();
        async move {
            let argv = ["tmplrmgr", "-q", "--rpc-url", rpc_url.as_str()]
                .into_iter()
                .map(str::to_owned)
                .chain(args)
                .collect::<Vec<_>>();
            let cli = Cli::try_parse_from(argv)?;
            let ctx = crate::context::build_context(&cli)?;
            crate::dispatch::dispatch(ctx, cli.command).await
        }
    };
    let strings = |args: &[&str]| args.iter().map(|arg| (*arg).to_owned()).collect::<Vec<_>>();

    run(strings(&[
        "account",
        "add-key",
        "--mpc-dao",
        dao.as_str(),
        "--mpc-contract",
        signer_id.as_str(),
        "--signer-id",
        controlled.as_str(),
        "--secret-key",
        &secret_key,
    ]))
    .await?;
    let installed = client
        .read(account::GetAccessKey {
            account_id: controlled.0.clone(),
            public_key: mpc_key.public_key().into(),
        })
        .await?;
    assert_eq!(
        installed.permission,
        account::AccessKeyPermission::FullAccess
    );

    let dir =
        std::env::temp_dir().join(format!("tmplrmgr-mpc-e2e-{}-{kind:?}", std::process::id()));
    std::fs::create_dir_all(&dir)?;
    let plan_path = dir.join("plan.json");
    let payload_path = dir.join("payload.json");
    let amount = NearToken::from_near(3);
    std::fs::write(
        &plan_path,
        serde_json::to_vec(&PlannedTransaction {
            signer_account_id: controlled.clone(),
            receiver_id: beneficiary.0.clone(),
            actions: vec![Action::Transfer(TransferAction { deposit: amount })],
            continue_on_failure: false,
        })?,
    )?;
    let kind_flag = match kind {
        PayloadKind::DelegateAction => "delegate-action",
        PayloadKind::Transaction => "transaction",
    };
    let mut propose = strings(&[
        "mpc",
        "propose",
        "--plan",
        plan_path.to_str().expect("utf-8 path"),
        "--dao",
        dao.as_str(),
        "--mpc-contract",
        signer_id.as_str(),
        "--kind",
        kind_flag,
        "--out",
        payload_path.to_str().expect("utf-8 path"),
        "--signer-id",
        proposer.as_str(),
        "--secret-key",
        &secret_key,
    ]);
    if blind {
        propose.push("--blind".to_owned());
    }
    run(propose).await?;

    let proposal: crate::mpc::dao::ProposalView = serde_json::from_value(
        client
            .read(contract::ViewFunction {
                contract_id: dao.clone(),
                method_name: ContractMethodName("get_proposal".to_owned()),
                args: ContractArgs::Json(serde_json::to_value(crate::mpc::dao::GetProposalArgs {
                    id: 0,
                })?),
            })
            .await?
            .value,
    )?;
    let in_description = Envelope::from_description(&proposal.description)?;
    assert_eq!(in_description.is_some(), !blind, "{}", proposal.description);
    let envelope = Envelope::read_file(&payload_path)?;
    if let Some(published) = in_description {
        assert_eq!(published, envelope);
    }
    let hash = envelope.payload.hash()?;
    assert_eq!(
        proposal.proposed_signature()?.request.payload.hash()?,
        hash,
        "the proposal asks for the hash of the payload it carries"
    );

    // Stand in for the MPC network: sign the hash the proposal carries.
    let signature = borsh::to_vec(&mpc_key.sign(near_api::CryptoHash(hash)))?;
    client
        .execute_as(
            proposer.clone(),
            tx::FunctionCall {
                receiver_id: signer_id.clone(),
                method_name: ContractMethodName("set_signature".to_owned()),
                args: ContractArgs::Json(serde_json::to_value(SetSignature {
                    payload_hex: hex::encode(hash),
                    response: SignatureResponseJson::Ed25519 {
                        signature: signature[1..].to_vec(),
                    },
                })?),
                gas: NearGas::from_tgas(30),
                deposit: NearToken::ZERO,
            },
        )
        .await?;

    let approve = client
        .execute_as(
            proposer.clone(),
            tx::FunctionCall {
                receiver_id: dao.clone(),
                method_name: ContractMethodName("act_proposal".to_owned()),
                args: ContractArgs::Json(serde_json::to_value(ActProposal {
                    id: 0,
                    action: "VoteApprove".to_owned(),
                })?),
                gas: NearGas::from_tgas(100),
                deposit: NearToken::ZERO,
            },
        )
        .await?;
    let approve_tx = approve
        .operation
        .latest_tx_hash()
        .expect("act_proposal was sent")
        .to_string();
    let approve_receipts = &approve
        .operation
        .final_outcome()
        .expect("act_proposal executed")
        .receipts;
    assert!(
        approve_receipts
            .iter()
            .all(|receipt| receipt.status == ReceiptStatus::Succeeded),
        "{approve_receipts:?}"
    );

    let before = client
        .read(account::Get {
            account_id: beneficiary.0.clone(),
        })
        .await?
        .amount;

    let mut show = strings(&["mpc", "show", "--dao", dao.as_str(), "--proposal-id", "0"]);
    let mut relay = strings(&[
        "mpc",
        "relay",
        "--dao",
        dao.as_str(),
        "--proposal-id",
        "0",
        "--tx-hash",
        &approve_tx,
        "--tx-signer",
        proposer.as_str(),
        "--signer-id",
        relayer.as_str(),
        "--secret-key",
        &secret_key,
    ]);
    if blind {
        let error = run(show.clone())
            .await
            .expect_err("a blind proposal cannot be reviewed without the payload file");
        assert!(error.to_string().contains("--payload-file"), "{error}");
        for args in [&mut show, &mut relay] {
            args.extend(strings(&[
                "--payload-file",
                payload_path.to_str().expect("utf-8 path"),
            ]));
        }
    }
    run(show).await?;
    run(relay).await?;

    let after = client
        .read(account::Get {
            account_id: beneficiary.0.clone(),
        })
        .await?
        .amount;
    assert_eq!(after, before.saturating_add(amount));

    std::fs::remove_dir_all(dir)?;
    Ok(())
}

#[derive(serde::Serialize)]
struct SetDerivedPublicKey {
    #[serde(flatten)]
    derivation: crate::mpc::signer_contract::DerivedPublicKeyArgs,
    public_key: String,
}

#[derive(serde::Serialize)]
struct SetSignature {
    payload_hex: String,
    response: SignatureResponseJson,
}

/// What the real MPC returns, written from the test's side.
#[derive(serde::Serialize)]
#[serde(tag = "scheme")]
enum SignatureResponseJson {
    Ed25519 { signature: Vec<u8> },
}

#[derive(serde::Serialize)]
struct ActProposal {
    id: u64,
    action: String,
}
