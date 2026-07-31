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

    let labelled =
        crate::dispatch::plan::build(&client, &alpha_market(), &public_key, &signer_id())
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
