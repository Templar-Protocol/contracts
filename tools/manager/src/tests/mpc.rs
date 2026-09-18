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

const EXECUTED: [&str; 8] = [
    "--dao",
    "dao.testnet",
    "--proposal-id",
    "7",
    "--tx-hash",
    "11111111111111111111111111111111",
    "--tx-signer",
    "voter.testnet",
];

/// A delegate action needs a fee payer; a signed transaction needs nobody, so
/// `broadcast` must parse with no credentials at all.
#[test]
fn relay_needs_a_relayer_and_broadcast_needs_none() {
    let relay = ["tmplrmgr", "mpc", "relay"].into_iter().chain(EXECUTED);
    assert!(Cli::try_parse_from(relay.clone()).is_err());
    let Command::Mpc {
        command: MpcNs::Relay(relay),
    } = Cli::try_parse_from(relay.chain(CREDS))
        .expect("relay parses with a relayer")
        .command
    else {
        panic!("expected Mpc::Relay")
    };
    assert_eq!(relay.executed.review.proposal_id, 7);
    assert_eq!(relay.executed.tx_signer.as_str(), "voter.testnet");

    let Command::Mpc {
        command: MpcNs::Broadcast(broadcast),
    } = Cli::try_parse_from(
        ["tmplrmgr", "mpc", "broadcast"]
            .into_iter()
            .chain(EXECUTED)
            .chain(["--print"]),
    )
    .expect("broadcast parses without a signer")
    .command
    else {
        panic!("expected Mpc::Broadcast")
    };
    assert_eq!(broadcast.executed.review.proposal_id, 7);
    assert!(broadcast.print);
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
        add_key.into_spec().permission
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
fn install_key_derives_at_the_conventional_path_by_default() {
    let Command::Mpc {
        command: MpcNs::InstallKey(install_key),
    } = Cli::try_parse_from(
        ["tmplrmgr", "mpc", "install-key", "--dao", "dao.testnet"]
            .into_iter()
            .chain(CREDS),
    )
    .expect("parses")
    .command
    else {
        panic!("expected Mpc::InstallKey")
    };
    assert_eq!(
        install_key
            .derivation
            .path_for(&install_key.signer.account_id()),
        format!("dao.testnet-{}", CREDS[1])
    );

    let error = Cli::try_parse_from(
        ["tmplrmgr", "mpc", "install-key", "--path", "custom"]
            .into_iter()
            .chain(CREDS),
    )
    .expect_err("--path without --dao");
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

/// A controlled account, a proposer, a mock MPC signer with a locally held
/// "derived" key, and a mock DAO — everything the `mpc` commands touch.
struct MpcFixture {
    harness: templar_gateway_testing::SandboxHarness,
    rpc_url: String,
    secret_key: String,
    controlled: templar_gateway_types::ManagedAccountId,
    proposer: templar_gateway_types::ManagedAccountId,
    signer_id: near_account_id::AccountId,
    dao: near_account_id::AccountId,
    /// Stands in for the MPC network: the test signs with it what the real
    /// network would sign with its derived key.
    mpc_key: near_api::SecretKey,
    dir: std::path::PathBuf,
}

impl MpcFixture {
    async fn start(label: &str, bond: templar_gateway_types::NearToken) -> anyhow::Result<Self> {
        use crate::mpc::signer_contract::{DerivedPublicKeyArgs, KeyType};

        let harness = templar_gateway_testing::SandboxHarness::start_owned().await?;
        let controlled = harness.create_user("mpc-target").await?;
        let proposer = harness.create_user("mpc-proposer").await?;
        let signer_id = harness.deploy_mock_signer("mpc-signer").await?;
        let dao = harness.deploy_mock_dao("mpc-dao", bond).await?;
        let mpc_key = near_api::signer::generate_secret_key()?;
        harness
            .call_function(
                &proposer,
                &signer_id,
                "set_derived_public_key",
                SetDerivedPublicKey {
                    derivation: DerivedPublicKeyArgs {
                        path: format!("{dao}-{}", *controlled),
                        predecessor: dao.clone(),
                        domain_id: KeyType::Ed25519.domain_id(),
                    },
                    public_key: mpc_key.public_key().to_string(),
                },
            )
            .await?;
        let dir = std::env::temp_dir().join(format!("tmplrmgr-mpc-{label}-{}", std::process::id()));
        std::fs::create_dir_all(&dir)?;
        Ok(Self {
            rpc_url: harness.network.rpc_endpoints[0].url.to_string(),
            secret_key: templar_gateway_testing::test_secret_key()?.to_string(),
            harness,
            controlled,
            proposer,
            signer_id,
            dao,
            mpc_key,
            dir,
        })
    }

    /// One `tmplrmgr` invocation against the sandbox, signed by `signer` if
    /// given (every harness account shares the fixed test key).
    async fn run(
        &self,
        signer: Option<&templar_gateway_types::ManagedAccountId>,
        args: &[&str],
    ) -> anyhow::Result<()> {
        let creds: &[&str] = match signer {
            Some(signer) => &[
                "--signer-id",
                signer.as_str(),
                "--secret-key",
                &self.secret_key,
            ],
            None => &[],
        };
        let invocation = ["tmplrmgr", "-q", "--rpc-url", &self.rpc_url]
            .into_iter()
            .chain(args.iter().copied())
            .chain(creds.iter().copied());
        let cli = Cli::try_parse_from(invocation)?;
        let ctx = crate::context::build_context(&cli)?;
        crate::dispatch::dispatch(ctx, cli.command).await
    }

    async fn install_key(&self) -> anyhow::Result<()> {
        self.run(
            Some(&self.controlled),
            &[
                "mpc",
                "install-key",
                "--dao",
                self.dao.as_str(),
                "--mpc-contract",
                self.signer_id.as_str(),
            ],
        )
        .await
    }

    /// Write a `--print json` plan transferring `amount` from the controlled
    /// account to `receiver`.
    fn plan(
        &self,
        receiver: &near_account_id::AccountId,
        amount: templar_gateway_types::NearToken,
    ) -> anyhow::Result<std::path::PathBuf> {
        use near_api::types::transaction::actions::{Action, TransferAction};

        let path = self.dir.join("plan.json");
        std::fs::write(
            &path,
            serde_json::to_vec(&templar_gateway_core::PlannedTransaction::single_action(
                self.controlled.clone(),
                receiver.clone(),
                Action::Transfer(TransferAction { deposit: amount }),
            ))?,
        )?;
        Ok(path)
    }

    async fn proposal(&self, id: u64) -> anyhow::Result<crate::mpc::dao::ProposalView> {
        self.harness
            .view_json(
                &self.dao,
                "get_proposal",
                crate::mpc::dao::GetProposalArgs { id },
            )
            .await
    }

    /// The "MPC" signs `hash`, so the next `sign` request for it succeeds.
    async fn mpc_signs(&self, hash: [u8; 32]) -> anyhow::Result<()> {
        let signature = borsh::to_vec(&self.mpc_key.sign(near_api::CryptoHash(hash)))?;
        self.harness
            .call_function(
                &self.proposer,
                &self.signer_id,
                "set_signature",
                SetSignature {
                    payload_hex: hex::encode(hash),
                    response: SignatureResponseJson::Ed25519 {
                        signature: signature[1..].to_vec(),
                    },
                },
            )
            .await?;
        Ok(())
    }

    /// Approve and execute proposal `id`; returns the vote's transaction hash.
    async fn approve(&self, id: u64) -> anyhow::Result<String> {
        use templar_gateway_types::operation::ReceiptStatus;

        let kind = self
            .harness
            .view_json::<serde_json::Value>(
                &self.dao,
                "get_proposal",
                crate::mpc::dao::GetProposalArgs { id },
            )
            .await?["kind"]
            .clone();
        let approve = self
            .harness
            .call_function(
                &self.proposer,
                &self.dao,
                "act_proposal",
                ActProposal {
                    id,
                    action: "VoteApprove",
                    proposal: kind,
                    memo: None,
                },
            )
            .await?;
        let receipts = &approve
            .operation
            .final_outcome()
            .expect("act_proposal executed")
            .receipts;
        assert!(
            receipts
                .iter()
                .all(|receipt| receipt.status == ReceiptStatus::Succeeded),
            "{receipts:?}"
        );
        Ok(approve
            .operation
            .latest_tx_hash()
            .expect("act_proposal was sent")
            .to_string())
    }

    async fn balance(
        &self,
        account: &near_account_id::AccountId,
    ) -> anyhow::Result<templar_gateway_types::NearToken> {
        Ok(self
            .harness
            .client()?
            .read(templar_gateway_methods_spec::account::Get {
                account_id: account.clone(),
            })
            .await?
            .amount)
    }
}

/// The whole operator flow against a mock MPC signer and a mock DAO: install
/// the derived key, propose, review before the vote, have the "MPC" sign the
/// hash the proposal carries, approve, review again, relay — and the
/// controlled account's transfer lands. Along the way, every refusal a voter
/// or relayer relies on: a payload that does not hash to the proposal, the
/// wrong send command for the kind, a replay, and a rejected nonce.
#[rstest::rstest]
#[case::public_delegate_action(PayloadKind::DelegateAction, false)]
#[case::blind_transaction(PayloadKind::Transaction, true)]
#[tokio::test]
async fn requires_sandbox_mpc_proposal_is_relayed_end_to_end(
    #[case] kind: PayloadKind,
    #[case] blind: bool,
) -> anyhow::Result<()> {
    use templar_gateway_methods_spec::account;
    use templar_gateway_types::{ContractMethodName, NearToken};

    use crate::mpc::envelope::Envelope;

    let f = MpcFixture::start(
        if blind { "blind" } else { "public" },
        NearToken::from_millinear(100),
    )
    .await?;
    let relayer = f.harness.create_user("mpc-relayer").await?;
    let beneficiary = f.harness.create_user("mpc-beneficiary").await?;
    let client = f.harness.client()?;

    f.install_key().await?;
    let installed = client
        .read(account::GetAccessKey {
            account_id: f.controlled.0.clone(),
            public_key: f.mpc_key.public_key().into(),
        })
        .await?;
    assert_eq!(
        installed.permission,
        account::AccessKeyPermission::FullAccess
    );

    // The plain key commands, on the same account, with a function-call key.
    let restricted_key = near_api::signer::generate_secret_key()?.public_key();
    let restricted = restricted_key.to_string();
    f.run(
        Some(&f.controlled),
        &[
            "account",
            "add-key",
            "--key",
            &restricted,
            "--receiver-id",
            f.dao.as_str(),
            "--method-name",
            "act_proposal",
        ],
    )
    .await?;
    assert_eq!(
        client
            .read(account::GetAccessKey {
                account_id: f.controlled.0.clone(),
                public_key: restricted_key.into(),
            })
            .await?
            .permission,
        account::AccessKeyPermission::FunctionCall {
            allowance: None,
            receiver_id: f.dao.clone(),
            method_names: vec![ContractMethodName("act_proposal".to_owned())],
        }
    );
    f.run(
        Some(&f.controlled),
        &["account", "delete-key", "--key", &restricted],
    )
    .await?;
    let deleted = client
        .read(account::GetAccessKey {
            account_id: f.controlled.0.clone(),
            public_key: restricted_key.into(),
        })
        .await
        .expect_err("the key is gone");
    assert!(
        deleted.to_string().contains("does not exist while viewing"),
        "review tells a missing key from an RPC failure by this text: {deleted}"
    );

    let amount = NearToken::from_near(3);
    let plan = f.plan(&beneficiary.0, amount)?;
    let payload_path = f.dir.join("payload.json");
    let payload_file = payload_path.to_str().expect("utf-8 path");
    let kind_flag = match kind {
        PayloadKind::DelegateAction => "delegate-action",
        PayloadKind::Transaction => "transaction",
    };
    let propose: &[&str] = &[
        "mpc",
        "propose",
        "--plan",
        plan.to_str().expect("utf-8 path"),
        "--dao",
        f.dao.as_str(),
        "--mpc-contract",
        f.signer_id.as_str(),
    ];

    let error = f
        .run(Some(&f.proposer), &[propose, &["--nonce", "1"]].concat())
        .await
        .expect_err("a nonce the chain would reject is refused before the vote");
    assert!(
        error.to_string().contains("below the key's next nonce"),
        "{error}"
    );

    let blind_flag: &[&str] = if blind { &["--blind"] } else { &[] };
    f.run(
        Some(&f.proposer),
        &[
            propose,
            &["--kind", kind_flag, "--out", payload_file],
            blind_flag,
        ]
        .concat(),
    )
    .await?;

    let proposal = f.proposal(0).await?;
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

    // The pieces every review command takes, named so a test can leave one out.
    let target: &[&str] = &["--dao", f.dao.as_str(), "--proposal-id", "0"];
    let mpc: &[&str] = &["--mpc-contract", f.signer_id.as_str()];
    let payload_arg: &[&str] = if blind {
        &["--payload-file", payload_file]
    } else {
        &[]
    };
    let show = [&["mpc", "show"][..], target, mpc, payload_arg].concat();
    if blind {
        let error = f
            .run(None, &[&["mpc", "show"][..], target, mpc].concat())
            .await
            .expect_err("a blind proposal cannot be reviewed without the payload file");
        assert!(error.to_string().contains("--payload-file"), "{error}");
    }
    // A voter reviews before voting: the proposal is still in progress.
    f.run(None, &show).await?;

    // Without `--mpc-contract` the review is pinned to the network's signer.
    let error = f
        .run(None, &[&["mpc", "show"][..], target, payload_arg].concat())
        .await
        .expect_err("the mock signer is not the network's MPC contract");
    assert!(error.to_string().contains("look-alike"), "{error}");

    // A payload that does not hash to what the proposal signs is refused,
    // whichever way it reaches the reviewer.
    let decoy_path = f.dir.join("decoy.json");
    Envelope::new(crate::mpc::payload::SignablePayload::build(
        crate::mpc::payload::BuildInputs {
            kind,
            planned: templar_gateway_core::PlannedTransaction::single_action(
                f.controlled.clone(),
                beneficiary.0.clone(),
                near_api::types::transaction::actions::Action::Transfer(
                    near_api::types::transaction::actions::TransferAction {
                        deposit: NearToken::from_near(300),
                    },
                ),
            ),
            public_key: f.mpc_key.public_key(),
            nonce: 58_000_001,
            block: client
                .read(templar_gateway_methods_spec::chain::GetBlock::default())
                .await?,
            valid_for_blocks: 1_000,
        },
    )?)
    .write_file(&decoy_path)?;
    let decoy: &[&str] = &["--payload-file", decoy_path.to_str().expect("utf-8 path")];
    let error = f
        .run(None, &[&["mpc", "show"][..], target, mpc, decoy].concat())
        .await
        .expect_err("a decoy payload is refused");
    assert!(error.to_string().contains("hashes to"), "{error}");

    f.mpc_signs(hash).await?;
    let approve_tx = f.approve(0).await?;
    f.run(None, &show).await?;

    let before = f.balance(&beneficiary.0).await?;
    let executed = [
        target,
        mpc,
        &["--tx-hash", &approve_tx, "--tx-signer", f.proposer.as_str()],
    ]
    .concat();
    // `relay` needs a fee payer; `broadcast` takes no credentials.
    let (send, sender, wrong, wrong_sender) = match kind {
        PayloadKind::DelegateAction => ("relay", Some(&relayer), "broadcast", None),
        PayloadKind::Transaction => ("broadcast", None, "relay", Some(&relayer)),
    };

    let error = f
        .run(
            wrong_sender,
            &[&["mpc", wrong][..], &executed, payload_arg].concat(),
        )
        .await
        .expect_err("the other send command refuses this payload kind");
    assert!(error.to_string().contains("use `mpc "), "{error}");

    let error = f
        .run(sender, &[&["mpc", send][..], &executed, decoy].concat())
        .await
        .expect_err("a decoy payload never reaches the signature");
    assert!(error.to_string().contains("hashes to"), "{error}");

    let send_args = [&["mpc", send][..], &executed, payload_arg].concat();
    f.run(sender, &send_args).await?;

    // The nonce is spent now: a replay is refused before anything is broadcast.
    let error = f
        .run(sender, &send_args)
        .await
        .expect_err("a relayed payload cannot be sent twice");
    assert!(
        error.to_string().contains("already signed at nonce"),
        "{error}"
    );

    assert_eq!(
        f.balance(&beneficiary.0).await?,
        before.saturating_add(amount)
    );
    std::fs::remove_dir_all(&f.dir)?;
    Ok(())
}

/// A payload past its validity is refused before any signature is looked up,
/// so an operator learns to re-propose rather than watching a broadcast fail.
#[tokio::test]
async fn requires_sandbox_mpc_expired_payload_is_refused_before_broadcast() -> anyhow::Result<()> {
    use templar_gateway_types::NearToken;

    let f = MpcFixture::start("expiry", NearToken::ZERO).await?;
    f.install_key().await?;
    let plan = f.plan(&f.proposer.0, NearToken::from_near(1))?;
    f.run(
        Some(&f.proposer),
        &[
            "mpc",
            "propose",
            "--plan",
            plan.to_str().expect("utf-8 path"),
            "--dao",
            f.dao.as_str(),
            "--mpc-contract",
            f.signer_id.as_str(),
            "--valid-for-blocks",
            "5",
        ],
    )
    .await?;

    let hash = f
        .proposal(0)
        .await?
        .proposed_signature()?
        .request
        .payload
        .hash()?;
    f.mpc_signs(hash).await?;
    let approve_tx = f.approve(0).await?;
    f.harness.fast_forward(20).await?;

    let error = f
        .run(
            Some(&f.proposer),
            &[
                "mpc",
                "relay",
                "--dao",
                f.dao.as_str(),
                "--proposal-id",
                "0",
                "--mpc-contract",
                f.signer_id.as_str(),
                "--tx-hash",
                &approve_tx,
                "--tx-signer",
                f.proposer.as_str(),
            ],
        )
        .await
        .expect_err("an approved but expired payload is refused before broadcast");
    assert!(error.to_string().contains("expired"), "{error}");

    std::fs::remove_dir_all(&f.dir)?;
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

/// Sputnik's `act_proposal` arguments: the kind is echoed back.
#[derive(serde::Serialize)]
struct ActProposal {
    id: u64,
    action: &'static str,
    proposal: serde_json::Value,
    memo: Option<String>,
}
