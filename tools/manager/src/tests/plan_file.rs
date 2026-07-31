//! The plan artifact (ENG-544): the deployment `market plan` writes and
//! `market apply` sends.
//!
//! The artifact layer is exercised offline against locally built transactions.
//! `build` itself is *not* offline — planning a registry deploy reads the
//! registry's source metadata — so the one test that calls it is named
//! `requires_network_*` and is excluded from both the fast and sandbox gates.

use std::path::Path;

use near_account_id::AccountId;
use near_api::types::transaction::actions::{Action, FunctionCallAction};
use near_api::types::NearToken;
use templar_gateway_client::{Client, Network, NetworkConfigBuilder};
use templar_gateway_core::{OperationPlan, PlannedTransaction};
use templar_gateway_types::primitive::PublicKey;
use templar_gateway_types::NearGas;

use crate::spec::{
    check::{Check, Status},
    plan::{Derived, PlanArgs, PlanFile},
    MarketSpec,
};

const PUBLIC_KEY: &str = "ed25519:H9k5eiU4xXS3M4z8HzKJSLaZdqGdGwBG49o7orNC4eZW";

fn alpha_market() -> MarketSpec {
    crate::spec::extends::load(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/spec/iethfxrp-ixlmusdc.toml"),
    )
    .expect("fixture spec should load")
}

fn signer_id() -> AccountId {
    "operator.near".parse().expect("valid account")
}

/// A transaction carrying one function call with the given args.
fn transaction(method_name: &str, args: Vec<u8>) -> PlannedTransaction {
    PlannedTransaction {
        signer_account_id: signer_id().into(),
        receiver_id: "templar-alpha.near".parse().expect("valid account"),
        actions: vec![Action::FunctionCall(Box::new(FunctionCallAction {
            method_name: method_name.to_owned(),
            args,
            gas: NearGas::from_tgas(300),
            deposit: NearToken::from_near(5),
        }))],
        continue_on_failure: false,
    }
}

/// Two JSON steps and one borsh step — the shapes a real deploy produces.
fn sample_steps() -> Vec<(String, PlannedTransaction)> {
    let json = serde_json::to_vec(&serde_json::json!({
        "registry_id": "templar-alpha.near",
        "name": "iethfxrp-ixlmusdc",
        "configuration": { "minimum_collateral_ratio": [3, 2] },
    }))
    .expect("encode json args");

    vec![
        (
            "deploy governance".to_owned(),
            transaction("deploy_market", json.clone()),
        ),
        (
            "deploy market".to_owned(),
            transaction("deploy_market", json),
        ),
        (
            "add a version".to_owned(),
            transaction("add_version", borsh_args()),
        ),
    ]
}

/// A borsh payload shape: a length-prefixed string, then bytes that are not
/// valid UTF-8. Probing, not a method-name list, is what must classify this.
fn borsh_args() -> Vec<u8> {
    [
        &6u32.to_le_bytes()[..],
        b"v1.3.0",
        &[0u8, 1, 2, 3, 250, 251, 252],
    ]
    .concat()
}

fn plan_file(steps: Vec<(String, PlannedTransaction)>) -> PlanFile {
    let spec = alpha_market();
    PlanFile::new(
        "mainnet".to_owned(),
        "sha256:test".to_owned(),
        Derived {
            market_id: spec.market_id().expect("market id"),
            oracle_id: spec.oracle_id().expect("oracle id"),
            governance_id: spec.governance_id().expect("governance id"),
            collateral_decimals: spec.collateral.decimals,
            borrow_decimals: spec.borrow.decimals,
        },
        vec![Check {
            id: "config.validate".to_owned(),
            status: Status::passed("MarketConfiguration::validate"),
        }],
        steps,
    )
    .expect("plan file should build")
}

/// The headline criterion: what `apply` sends is exactly what `plan` showed.
///
/// Compared after conversion into the file, because that conversion
/// canonicalizes JSON args. Asserting on the pre-canonical bytes would assert on
/// `serde_json`'s key ordering, not on anything a contract can observe.
#[test]
fn round_trips_through_json() {
    let file = plan_file(sample_steps());

    let text = serde_json::to_string_pretty(&file).expect("serialize");
    let parsed: PlanFile = serde_json::from_str(&text).expect("deserialize");

    assert_eq!(parsed, file, "the artifact must survive a JSON round trip");
    assert_eq!(
        parsed
            .clone()
            .into_operation_plan()
            .expect("parsed plan converts"),
        file.into_operation_plan().expect("original plan converts"),
        "the transactions sent must be the transactions planned"
    );
}

/// JSON args stay editable; borsh stays opaque and survives byte-for-byte.
#[test]
fn args_are_classified_by_probing_the_bytes() {
    let file = plan_file(sample_steps());

    assert!(
        matches!(file.steps[0].function_calls[0].args, PlanArgs::Json(_)),
        "JSON args must stay editable: {:?}",
        file.steps[0].function_calls[0].args
    );
    assert!(
        matches!(file.steps[2].function_calls[0].args, PlanArgs::Base64(_)),
        "borsh must not be mistaken for JSON: {:?}",
        file.steps[2].function_calls[0].args
    );

    let text = serde_json::to_string(&file).expect("serialize");
    let parsed: PlanFile = serde_json::from_str(&text).expect("deserialize");
    let restored = parsed.into_operation_plan().expect("convert back");

    assert_eq!(
        restored.steps[2].actions,
        transaction("add_version", borsh_args()).actions,
        "opaque args must survive byte-for-byte"
    );
}

/// Editing is the feature, so an edit is reported rather than refused — and
/// named precisely enough to confirm it was the only one.
#[test]
fn an_edited_plan_names_the_steps_that_changed() {
    let mut file = plan_file(sample_steps());
    assert!(
        file.drift().expect("digest").is_clean(),
        "a freshly generated plan is unmodified"
    );

    file.steps[1].function_calls[0].gas += 1;
    let drift = file.drift().expect("digest");

    assert_eq!(drift.changed, vec![1], "only step 1 was touched");
    assert_eq!(drift.delta, 0);
    assert!(
        drift.describe().contains("1 step(s) differ (#1)"),
        "the operator must be told which step: {}",
        drift.describe()
    );
}

/// Removing a step is an edit too, and must not read as unmodified.
#[test]
fn a_removed_step_is_reported() {
    let mut file = plan_file(sample_steps());
    file.steps.pop();

    let drift = file.drift().expect("digest");
    assert_eq!(drift.delta, -1);
    assert!(!drift.is_clean());
    assert!(
        drift.describe().contains("1 step(s) removed"),
        "{}",
        drift.describe()
    );
}

/// Editing a check from `failed` to `passed`, or repointing a derived id, must
/// not report the plan as unmodified — `render` shows those values, so a clean
/// verdict over them is worth more than it should be.
#[test]
fn editing_the_summary_is_drift_too() {
    let mut file = plan_file(sample_steps());

    file.checks[0].status = Status::failed("actually broken");
    let drift = file.drift().expect("digest");
    assert!(drift.summary, "a rewritten check verdict is an edit");
    assert!(!drift.is_clean());
    assert!(
        drift.describe().contains("check results differ"),
        "{}",
        drift.describe()
    );

    let mut file = plan_file(sample_steps());
    file.derived.market_id = "elsewhere.near".parse().expect("valid account");
    assert!(
        file.drift().expect("digest").summary,
        "a repointed market id is an edit"
    );

    // `render` presents `spec_digest` as the plan's source, so rewriting it
    // must not read as unmodified either.
    let mut file = plan_file(sample_steps());
    file.spec_digest = "sha256:something-else".to_owned();
    assert!(
        file.drift().expect("digest").summary,
        "a rewritten spec digest is an edit"
    );
}

/// A number too large for `u64` decodes as `f64` and would re-encode in
/// exponent form — a different value than the operator reviewed. Such args stay
/// opaque instead.
#[test]
fn args_that_would_not_survive_re_encoding_stay_opaque() {
    let lossy = br#"{"amount":123456789012345678901234567890}"#.to_vec();
    let file = plan_file(vec![(
        "big number".to_owned(),
        transaction("deposit", lossy.clone()),
    )]);

    assert!(
        matches!(file.steps[0].function_calls[0].args, PlanArgs::Base64(_)),
        "a value that cannot round-trip must not be presented as editable JSON"
    );
    assert_eq!(
        file.into_operation_plan().expect("convert").steps[0].actions,
        transaction("deposit", lossy).actions,
        "and it must survive byte-for-byte"
    );
}

/// A step that tolerates its own failure would let a hand-edit turn a reverted
/// governance call into `apply` exiting zero.
#[test]
fn a_failure_tolerating_step_is_refused() {
    let mut tolerant = transaction("deploy_market", b"{}".to_vec());
    tolerant.continue_on_failure = true;

    let error = PlanFile::new(
        "mainnet".to_owned(),
        "sha256:test".to_owned(),
        Derived {
            market_id: "m.near".parse().expect("valid account"),
            oracle_id: "o.near".parse().expect("valid account"),
            governance_id: "g.near".parse().expect("valid account"),
            collateral_decimals: Some(6),
            borrow_decimals: Some(7),
        },
        Vec::new(),
        vec![("tolerant".to_owned(), tolerant)],
    )
    .expect_err("a deployment must not tolerate a reverted step");

    assert!(
        format!("{error:#}").contains("tolerates its own failure"),
        "{error:#}"
    );
}

/// A plan carrying an action kind the artifact cannot render is refused, not
/// silently dropped — a dropped action would be a transaction the operator
/// reviewed and the chain never saw.
#[test]
fn a_non_function_call_action_is_refused() {
    let plan = OperationPlan {
        steps: vec![PlannedTransaction {
            signer_account_id: signer_id().into(),
            receiver_id: "market.near".parse().expect("valid account"),
            actions: vec![Action::DeleteAccount(
                near_api::types::transaction::actions::DeleteAccountAction {
                    beneficiary_id: "beneficiary.near".parse().expect("valid account"),
                },
            )],
            continue_on_failure: false,
        }],
    };

    let error = PlanFile::new(
        "mainnet".to_owned(),
        "sha256:test".to_owned(),
        Derived {
            market_id: "m.near".parse().expect("valid account"),
            oracle_id: "o.near".parse().expect("valid account"),
            governance_id: "g.near".parse().expect("valid account"),
            collateral_decimals: Some(6),
            borrow_decimals: Some(7),
        },
        Vec::new(),
        vec![("tear down".to_owned(), plan.steps[0].clone())],
    )
    .expect_err("a delete-account action cannot be rendered");

    assert!(
        format!("{error:#}").contains("only function calls"),
        "{error:#}"
    );
}

/// Planning for an account that will not hold the Admin role must fail before
/// anything is planned, not after 8.5 NEAR of deploys have succeeded and every
/// proposal reverts. Offline: the guard runs before the first read.
#[tokio::test]
async fn a_signer_that_is_not_the_governance_admin_is_refused() {
    let client = Client::builder(NetworkConfigBuilder::new(Network::Mainnet).build())
        .build()
        .expect("build client");
    let public_key = PublicKey::from(
        PUBLIC_KEY
            .parse::<near_api::PublicKey>()
            .expect("valid key"),
    );
    let spec = alpha_market();
    assert_ne!(
        spec.governance.admin,
        signer_id(),
        "the fixture must not already agree, or this proves nothing"
    );

    let error = crate::dispatch::plan::build(&client, &spec, &public_key, &signer_id())
        .await
        .expect_err("a non-admin signer cannot execute the proxy proposals");

    assert!(
        format!("{error:#}").contains("would not hold the Admin role"),
        "{error:#}"
    );
}

/// A proxy-oracle version whose `new` ignores `owner_id` leaves the *registry*
/// as owner, so governance can never configure either proxy. Because
/// `admin_set_proxy` is dispatched detached, the proposals would still report
/// success and the deploy would reach market creation with a dead oracle — so
/// this is refused at plan time. Offline: the guard precedes the first read.
#[tokio::test]
async fn an_oracle_version_that_ignores_owner_id_is_refused() {
    let client = Client::builder(NetworkConfigBuilder::new(Network::Mainnet).build())
        .build()
        .expect("build client");
    let public_key = PublicKey::from(
        PUBLIC_KEY
            .parse::<near_api::PublicKey>()
            .expect("valid key"),
    );
    let mut spec = alpha_market();
    let admin = spec.governance.admin.clone();
    // Well-formed key, pre-0.3.0 version: the guard must reject it for what the
    // version *means*, not because the key failed to parse.
    spec.versions.proxy_oracle = spec.versions.proxy_oracle.replace("@0.3.0#", "@0.2.0#");

    let error = crate::dispatch::plan::build(&client, &spec, &public_key, &admin)
        .await
        .expect_err("a pre-0.3.0 oracle cannot seat an owner");

    assert!(
        format!("{error:#}").contains("registry would own the oracle"),
        "{error:#}"
    );
}

/// The deployment `deploy.sh` performs, in the order it performs it.
///
/// The order is a safety property: `registry deploy` fails when the account
/// already exists, so governance must be created before the oracle names it as
/// owner, and both before the market points at the oracle.
///
/// Needs the network: planning a registry deploy reads the registry's contract
/// source metadata to pick the init-args encoding.
#[tokio::test]
async fn requires_network_plans_the_deploy_script_in_order() {
    let client = Client::builder(NetworkConfigBuilder::new(Network::Mainnet).build())
        .build()
        .expect("build client");
    let public_key = PublicKey::from(
        PUBLIC_KEY
            .parse::<near_api::PublicKey>()
            .expect("valid key"),
    );

    // Signed by the spec's `governance.admin`: any other account would not hold
    // the Admin role, and `build` refuses that rather than plan a deployment
    // whose proposals revert after the deposits are already spent.
    let spec = alpha_market();
    let admin = spec.governance.admin.clone();
    let labelled = crate::dispatch::plan::build(&client, &spec, &public_key, &admin)
        .await
        .expect("the alpha fixture should plan");

    let sequence: Vec<_> = labelled
        .iter()
        .map(|(label, transaction)| {
            let method = match transaction.actions.as_slice() {
                [Action::FunctionCall(call)] => call.method_name.as_str(),
                other => panic!("expected one function call, got {other:?}"),
            };
            (transaction.receiver_id.as_str(), method, label.as_str())
        })
        .collect();

    // The three registry deploys share one method (`deploy_market`) and one
    // receiver, so the label is what identifies each — asserted for that reason.
    assert_eq!(
        sequence,
        vec![
            (
                "templar-alpha.near",
                "deploy_market",
                "deploy governance proxy-gov-iethfxrp-ixlmusdc.templar-alpha.near"
            ),
            (
                "templar-alpha.near",
                "deploy_market",
                "deploy proxy oracle proxy-oracle-iethfxrp-ixlmusdc.templar-alpha.near, \
                 owned by governance"
            ),
            (
                "proxy-gov-iethfxrp-ixlmusdc.templar-alpha.near",
                "create_proposal",
                "propose collateral proxy (proposal 0)"
            ),
            (
                "proxy-gov-iethfxrp-ixlmusdc.templar-alpha.near",
                "execute_proposal",
                "execute collateral proxy proposal 0"
            ),
            (
                "proxy-gov-iethfxrp-ixlmusdc.templar-alpha.near",
                "create_proposal",
                "propose borrow proxy (proposal 1)"
            ),
            (
                "proxy-gov-iethfxrp-ixlmusdc.templar-alpha.near",
                "execute_proposal",
                "execute borrow proxy proposal 1"
            ),
            (
                "templar-alpha.near",
                "deploy_market",
                "deploy market iethfxrp-ixlmusdc.templar-alpha.near"
            ),
        ],
        "the plan must reproduce deploy.sh"
    );
}

/// Journal reconciliation (ENG-546). All offline: the journal keys on the plan's
/// own per-step digests, so nothing here needs a chain.
mod journal {
    use super::{plan_file, sample_steps};
    use crate::spec::journal::{Entry, Journal};

    fn entry(step: usize, digest: String) -> Entry {
        Entry {
            step,
            digest,
            label: format!("step {step}"),
            outcome: crate::spec::journal::Outcome::Completed,
            tx_hash: None,
        }
    }

    fn done(file: &crate::spec::plan::PlanFile, steps: &[usize]) -> Journal {
        Journal {
            entries: steps
                .iter()
                .map(|index| {
                    entry(
                        *index,
                        crate::spec::journal::executable_digest(&file.steps[*index])
                            .expect("digest"),
                    )
                })
                .collect(),
        }
    }

    /// The point of the journal: an interrupted deploy continues instead of
    /// repeating work that already spent its deposit.
    #[test]
    fn completed_steps_are_skipped() {
        let file = plan_file(sample_steps());
        let journal = done(&file, &[0]);

        assert_eq!(
            journal.remaining(&file).expect("reconciles"),
            vec![1, 2],
            "only the steps that have not run"
        );
    }

    #[test]
    fn a_fresh_journal_runs_everything() {
        let file = plan_file(sample_steps());
        assert_eq!(
            Journal::default().remaining(&file).expect("reconciles"),
            vec![0, 1, 2]
        );
    }

    /// Editing a step that has *already run* is the case that must be refused:
    /// re-running it repeats a completed transaction, skipping it applies
    /// something nobody executed. Named rather than silently resolved either way.
    #[test]
    fn an_edit_under_a_completed_step_is_refused() {
        let mut file = plan_file(sample_steps());
        let journal = done(&file, &[0]);
        file.steps[0].function_calls[0].gas += 1;

        let error = journal
            .remaining(&file)
            .expect_err("the plan changed under a completed step");
        assert!(
            format!("{error:#}").contains("changed under it"),
            "{error:#}"
        );
    }

    /// Editing a step that has *not* run is fine — the artifact exists to be
    /// edited, and only completed steps are frozen.
    #[test]
    fn an_edit_ahead_of_the_cursor_is_allowed() {
        let mut file = plan_file(sample_steps());
        let journal = done(&file, &[0]);
        file.steps[2].function_calls[0].gas += 1;

        assert_eq!(journal.remaining(&file).expect("reconciles"), vec![1, 2]);
    }

    /// A journal from a different plan must not be mistaken for progress on
    /// this one.
    #[test]
    fn a_journal_for_another_plan_is_refused() {
        let file = plan_file(sample_steps());
        let journal = Journal {
            entries: vec![entry(99, "sha256:whatever".to_owned())],
        };

        let error = journal.remaining(&file).expect_err("out of range");
        assert!(format!("{error:#}").contains("different plan"), "{error:#}");
    }

    /// A step that was submitted but never resolved is neither progress nor
    /// absence: re-sending a deploy that actually landed strands its deposit,
    /// skipping it deploys nothing. Only a human can tell, so the run stops.
    #[test]
    fn an_unresolved_attempt_stops_the_run() {
        use crate::spec::journal::Outcome;

        let file = plan_file(sample_steps());
        let mut journal = done(&file, &[0]);
        journal.entries[0].outcome = Outcome::Attempted;
        journal.entries[0].tx_hash = Some("SOMEHASH".to_owned());

        let error = journal.remaining(&file).expect_err("outcome unknown");
        let rendered = format!("{error:#}");
        assert!(rendered.contains("never recorded"), "{rendered}");
        assert!(rendered.contains("SOMEHASH"), "{rendered}");
    }

    /// Steps run in order and only completions are recorded, so the done set is
    /// always a prefix. A gap means an entry was removed or invented — and
    /// resuming would step over work that never happened while every
    /// plan-level check still passed, because the *plan* is intact.
    #[test]
    fn a_journal_with_a_gap_is_refused() {
        let file = plan_file(sample_steps());
        let journal = done(&file, &[0, 2]);

        let error = journal.remaining(&file).expect_err("not a prefix");
        assert!(format!("{error:#}").contains("not a prefix"), "{error:#}");
    }

    /// Renaming a completed step changes nothing executable, so it must not
    /// block a resume.
    #[test]
    fn relabelling_a_completed_step_does_not_block_resume() {
        let mut file = plan_file(sample_steps());
        let journal = done(&file, &[0]);
        file.steps[0].label = "renamed by hand".to_owned();

        assert_eq!(journal.remaining(&file).expect("reconciles"), vec![1, 2]);
    }

    /// The showstopper this review caught: a resume has already created the
    /// accounts its completed steps made, so checking the *whole* plan's targets
    /// for freeness aborts every resume that has anything to resume. Freeness
    /// must be asked of the outstanding steps only — and asking it of those is
    /// what keeps the guard biting when a step's outcome was ambiguous and it
    /// therefore stayed outstanding.
    #[test]
    fn freeness_is_asked_only_of_the_steps_still_to_run() {
        use crate::spec::plan::{PlanArgs, PlanFile, PlanFunctionCall, PlanStep};

        let deploy = |name: &str| PlanStep {
            label: format!("deploy {name}"),
            signer_id: "operator.near".parse().expect("valid account"),
            receiver_id: "templar-alpha.near".parse().expect("valid account"),
            function_calls: vec![PlanFunctionCall {
                method_name: "deploy_market".to_owned(),
                args: PlanArgs::Json(serde_json::json!({
                    "name": name, "version_key": "v1",
                })),
                gas: 300_000_000_000_000,
                deposit: near_api::types::NearToken::from_near(5),
            }],
        };

        let mut file = plan_file(sample_steps());
        file.steps = vec![deploy("gov"), deploy("oracle"), deploy("market")];

        let all = crate::dispatch::plan::planned_targets(&file).expect("targets");
        assert_eq!(all.len(), 3, "three deploys, three accounts");

        // Step 0 completed, so its target already exists and must not be asked
        // to be free.
        let outstanding = PlanFile {
            steps: file.steps[1..].to_vec(),
            ..file.clone()
        };
        let remaining_targets =
            crate::dispatch::plan::planned_targets(&outstanding).expect("targets");

        assert!(
            remaining_targets.len() < all.len(),
            "a completed deploy's target must drop out of the freeness check: \
             {all:?} vs {remaining_targets:?}"
        );
    }

    /// The journal lives beside its plan, derived rather than configurable.
    #[test]
    fn the_journal_sits_beside_its_plan() {
        assert_eq!(
            crate::spec::journal::path_for(std::path::Path::new("/tmp/mkt/plan.json")),
            std::path::PathBuf::from("/tmp/mkt/plan.json.journal.json")
        );
    }
}
