//! The plan artifact (ENG-544): the deployment `market plan` writes and
//! `market apply` sends.
//!
//! The artifact layer is exercised offline against locally built transactions.
//! `build` itself is *not* offline — planning a registry deploy reads the
//! registry's source metadata — so the network-dependent tests are named
//! `requires_network_*` and excluded from both the fast and sandbox gates.

use near_account_id::AccountId;
use near_api::types::transaction::actions::{Action, FunctionCallAction};
use near_api::types::NearToken;
use templar_common::Nanoseconds;
use templar_gateway_client::{Client, Network, NetworkConfigBuilder};
use templar_gateway_core::{OperationPlan, PlannedTransaction};
use templar_gateway_types::NearGas;

use crate::spec::{
    check::{Check, Status},
    plan::{
        testing::{alpha_market, public_key},
        DeploymentStage, PlanArgs, PlanFile,
    },
};
use crate::spec::{OracleMode, BORROW_PRICE_ID, COLLATERAL_PRICE_ID};
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
    PlanFile::new(
        alpha_market(),
        public_key(),
        vec![Check {
            id: "config.validate".to_owned(),
            status: Status::passed("MarketConfiguration::validate"),
        }],
        steps,
    )
    .expect("plan file should build")
}

/// The artifact is persisted and read back by a later run, so its shape is a
/// compatibility surface: every struct in it is `deny_unknown_fields`, and a
/// field added or removed without a version bump makes an interrupted
/// deployment unresumable in both directions.
///
/// Pinning the whole nested key set — the spec travels inside the plan — turns
/// that into a test failure here rather than an opaque "unknown field" in front
/// of an operator mid-deploy.
#[test]
fn the_plan_shape_is_pinned_to_its_version() {
    fn key_paths(value: &serde_json::Value, path: &str, into: &mut Vec<String>) {
        match value {
            serde_json::Value::Object(fields) => {
                for (key, value) in fields {
                    let path = if path.is_empty() {
                        key.clone()
                    } else {
                        format!("{path}.{key}")
                    };
                    into.push(path.clone());
                    key_paths(value, &path, into);
                }
            }
            // One element is enough: every element of a homogeneous list has
            // the same shape, and indices would make the pin order-dependent.
            serde_json::Value::Array(items) => {
                if let Some(first) = items.first() {
                    key_paths(first, &format!("{path}[]"), into);
                }
            }
            _ => {}
        }
    }

    let rendered = serde_json::to_value(plan_file(sample_steps())).expect("serialize");
    let mut paths = Vec::new();
    key_paths(&rendered, "", &mut paths);
    paths.sort();

    // Hashed directly, not through `plan::digest`: that carries the journal's
    // format tag, and renaming it for journal reasons must not fail this.
    let fingerprint = templar_contract_artifacts::sha256_hex(paths.join("\n").as_bytes());
    assert_eq!(
        (crate::spec::plan::PLAN_SCHEMA_VERSION, fingerprint.as_str()),
        (
            2,
            "25391b8db980bcab36c9133335144b6a0f859a783f615618d1bad6e01bbf3285"
        ),
        "the plan artifact's shape changed. Update this pin — and once the tool \
         has shipped, bump PLAN_SCHEMA_VERSION with it, so a plan written by \
         another build is refused by name rather than failing on an unknown \
         field.\n\nshape:\n{}",
        paths.join("\n"),
    );
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

/// JSON args stay legible; borsh stays opaque and survives byte-for-byte.
#[test]
fn args_are_classified_by_probing_the_bytes() {
    let file = plan_file(sample_steps());

    assert!(
        matches!(file.steps[0].function_calls[0].args, PlanArgs::Json(_)),
        "JSON args must stay legible: {:?}",
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
        "a value that cannot round-trip must not be presented as plain JSON"
    );
    assert_eq!(
        file.into_operation_plan().expect("convert").steps[0].actions,
        transaction("deposit", lossy).actions,
        "and it must survive byte-for-byte"
    );
}

/// A step that tolerates its own failure would turn a reverted governance call
/// into `apply` exiting zero.
#[test]
fn a_failure_tolerating_step_is_refused() {
    let mut tolerant = transaction("deploy_market", b"{}".to_vec());
    tolerant.continue_on_failure = true;

    let error = PlanFile::new(
        alpha_market(),
        public_key(),
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
        alpha_market(),
        public_key(),
        Vec::new(),
        vec![("tear down".to_owned(), plan.steps[0].clone())],
    )
    .expect_err("a delete-account action cannot be rendered");

    assert!(
        format!("{error:#}").contains("only function calls"),
        "{error:#}"
    );
}

/// Direct-oracle specs cannot produce an empty prefix for a proxy boundary.
#[tokio::test]
async fn direct_oracle_specs_reject_proxy_stage_boundaries() {
    let client = Client::builder(NetworkConfigBuilder::new(Network::Testnet).build())
        .build()
        .expect("build client");
    let public_key = public_key();
    let mut spec = alpha_market();
    spec.oracle = OracleMode::Direct {
        account_id: "pyth-oracle.near".parse().unwrap(),
    };
    spec.collateral.price_id = Some(COLLATERAL_PRICE_ID);
    spec.borrow.price_id = Some(BORROW_PRICE_ID);

    let error = crate::dispatch::plan::build(
        &client,
        &spec,
        &public_key,
        &signer_id(),
        false,
        DeploymentStage::Governance,
    )
    .await
    .expect_err("direct markets have no proxy stages");
    assert!(format!("{error:#}").contains("no proxy deployment stage"));
}

/// Planning for an account that will not hold the Admin role must fail before
/// anything is planned, not after 8.5 NEAR of deploys have succeeded and every
/// proposal reverts. Offline: the guard runs before the first read.
#[tokio::test]
async fn a_signer_that_is_not_the_governance_admin_is_refused() {
    let client = Client::builder(NetworkConfigBuilder::new(Network::Mainnet).build())
        .build()
        .expect("build client");
    let public_key = public_key();
    let spec = alpha_market();
    assert_ne!(
        spec.proxy().expect("proxy fixture").0.admin,
        signer_id(),
        "the fixture must not already agree, or this proves nothing"
    );

    let error = crate::dispatch::plan::build(
        &client,
        &spec,
        &public_key,
        &signer_id(),
        false,
        DeploymentStage::Market,
    )
    .await
    .expect_err("a non-admin signer cannot execute the proxy proposals");

    assert!(
        format!("{error:#}").contains("would not hold the Admin role"),
        "{error:#}"
    );
}

#[tokio::test]
async fn proxy_configuration_rejects_nonzero_ttl() {
    let client = Client::builder(NetworkConfigBuilder::new(Network::Testnet).build())
        .build()
        .expect("build client");
    let public_key = public_key();
    let mut spec = alpha_market();
    let crate::spec::OracleMode::Proxy { governance, .. } = &mut spec.oracle else {
        panic!("the fixture must be a proxy market");
    };
    governance.ttl_default = Nanoseconds::from_secs(1);

    let error = crate::dispatch::plan::build(
        &client,
        &spec,
        &public_key,
        &signer_id(),
        true,
        DeploymentStage::ProxyConfiguration,
    )
    .await
    .expect_err("a proxy configuration plan cannot wait");

    assert!(
        format!("{error:#}").contains("a plan cannot wait"),
        "{error:#}"
    );
}

#[tokio::test]
async fn requires_network_proposal_free_stages_allow_deferred_constraints() {
    let client = Client::builder(NetworkConfigBuilder::new(Network::Mainnet).build())
        .build()
        .expect("build client");
    let public_key = public_key();
    let mut spec = alpha_market();
    let crate::spec::OracleMode::Proxy { governance, .. } = &mut spec.oracle else {
        panic!("the fixture must be a proxy market");
    };
    governance.ttl_default = Nanoseconds::from_secs(1);

    for (stage, length) in [
        (DeploymentStage::Governance, 1),
        (DeploymentStage::ProxyOracle, 2),
    ] {
        let steps =
            crate::dispatch::plan::build(&client, &spec, &public_key, &signer_id(), true, stage)
                .await
                .expect("deployment-only stages do not require proposal constraints");
        assert_eq!(steps.len(), length, "{stage:?} must emit its prefix");
    }
}

#[tokio::test]
async fn requires_network_governance_stage_does_not_resolve_proxy_version() {
    let client = Client::builder(NetworkConfigBuilder::new(Network::Mainnet).build())
        .build()
        .expect("build client");
    let public_key = public_key();
    let mut spec = alpha_market();
    let crate::spec::OracleMode::Proxy { oracle_version, .. } = &mut spec.oracle else {
        panic!("the fixture must be a proxy market");
    };
    *oracle_version =
        "missing-proxy@999.0.0#abababababababababababababababababababababababababababababababab"
            .to_owned();
    let admin = spec.proxy().expect("proxy fixture").0.admin.clone();

    let steps = crate::dispatch::plan::build(
        &client,
        &spec,
        &public_key,
        &admin,
        false,
        DeploymentStage::Governance,
    )
    .await
    .expect("governance planning must not resolve the later proxy version");

    assert_eq!(steps.len(), 1);
    assert!(steps[0].0.starts_with("deploy governance "));
}

/// The full proxy deployment, in order.
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
    let public_key = public_key();

    // Signed by the spec's `governance.admin`: any other account would not hold
    // the Admin role, and `build` refuses that rather than plan a deployment
    // whose proposals revert after the deposits are already spent.
    let spec = alpha_market();
    let admin = spec.proxy().expect("proxy fixture").0.admin.clone();
    let labeled = crate::dispatch::plan::build(
        &client,
        &spec,
        &public_key,
        &admin,
        true,
        DeploymentStage::Market,
    )
    .await
    .expect("the alpha fixture should plan");

    let sequence: Vec<_> = labeled
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
        "the plan must deploy governance, then the oracle it owns, then the market"
    );

    for (stage, length) in [
        (DeploymentStage::Governance, 1),
        (DeploymentStage::ProxyOracle, 2),
        (DeploymentStage::ProxyConfiguration, 6),
    ] {
        let staged = crate::dispatch::plan::build(&client, &spec, &public_key, &admin, true, stage)
            .await
            .expect("the proxy prefix should plan");
        assert_eq!(
            staged,
            labeled[..length],
            "{stage:?} must retain the full plan's dependency-ordered prefix"
        );
    }
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
            entries: vec![entry(
                99,
                crate::spec::plan::digest(b"a step of some other plan").to_string(),
            )],
        };

        let error = journal.remaining(&file).expect_err("out of range");
        assert!(format!("{error:#}").contains("different plan"), "{error:#}");
    }

    /// A build that hashes steps differently produces digests that match
    /// nothing. Named as the format difference it is, rather than reported as
    /// every completed step having changed under the operator.
    #[test]
    fn a_journal_from_a_build_with_another_digest_format_is_refused() {
        let file = plan_file(sample_steps());
        let journal = Journal {
            entries: vec![entry(0, "sha256:from-an-older-build".to_owned())],
        };

        let error = journal.remaining(&file).expect_err("foreign digest format");
        assert!(
            format!("{error:#}").contains("different build"),
            "{error:#}"
        );
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

    /// A conditional step that has already run drops out of the re-derivation,
    /// and labels carry an `(n/total)` counter — so every step after it is
    /// renumbered without any of them doing anything different. Comparing the
    /// label would refuse the resume and strand a market mid-registration.
    #[test]
    fn a_renumbered_label_does_not_block_resume() {
        let file = plan_file(sample_steps());

        // Step 1 is the completed registration the gateway no longer plans; the
        // step after it keeps its work and loses its place in the numbering.
        let mut expected: Vec<_> = file.steps.clone();
        expected.remove(1);
        for (index, step) in expected.iter_mut().enumerate() {
            step.label = format!("deploy market ({}/2)", index + 1);
        }

        crate::dispatch::plan::ensure_matches_spec(&file, &expected, &[2])
            .expect("only the labels moved");
    }

    /// A resume has already created the accounts its completed steps made, so
    /// checking the *whole* plan's targets for freeness aborts every resume that
    /// has anything to resume. Freeness is asked of the outstanding steps only,
    /// which also keeps it biting on a step whose outcome was ambiguous.
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

        let all = crate::dispatch::plan::planned_targets(&file.steps).expect("targets");
        let names = |targets: &[near_api::AccountId]| -> Vec<String> {
            targets.iter().map(ToString::to_string).collect()
        };
        assert_eq!(
            names(&all),
            [
                "gov.templar-alpha.near",
                "oracle.templar-alpha.near",
                "market.templar-alpha.near"
            ]
        );

        // Step 0 completed, so its target already exists and must not be asked
        // to be free.
        let outstanding = PlanFile {
            steps: file.steps[1..].to_vec(),
            ..file.clone()
        };
        let remaining_targets =
            crate::dispatch::plan::planned_targets(&outstanding.steps).expect("targets");

        // The exact set, not a smaller count: `len() < 3` also passes for zero,
        // and an under-broad freeness check is the dangerous direction — it lets
        // `apply` reach a registry deploy against an account that already
        // exists, after the earlier deposits are spent.
        assert_eq!(
            names(&remaining_targets),
            ["oracle.templar-alpha.near", "market.templar-alpha.near"],
            "exactly the completed step's target drops out"
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
