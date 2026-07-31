//! The plan artifact (ENG-544): the deployment `market plan` writes and
//! `market apply` sends.
//!
//! All offline. Planning these writes needs no network, so the whole
//! spec → plan → file → plan path is exercised without a node.

use std::path::Path;

use near_account_id::AccountId;
use templar_gateway_client::{Client, Network, NetworkConfigBuilder};
use templar_gateway_core::{OperationPlan, PlannedTransaction};
use templar_gateway_types::primitive::PublicKey;

use crate::spec::{
    check::{Check, Status},
    plan::{Derived, PlanArgs, PlanFile},
    MarketSpec,
};

const PUBLIC_KEY: &str = "ed25519:H9k5eiU4xXS3M4z8HzKJSLaZdqGdGwBG49o7orNC4eZW";

fn offline_client() -> Client {
    Client::builder(NetworkConfigBuilder::new(Network::Mainnet).build())
        .build()
        .expect("build offline client")
}

fn alpha_market() -> MarketSpec {
    crate::spec::extends::load(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/spec/iethfxrp-ixlmusdc.toml"),
    )
    .expect("fixture spec should load")
}

fn signer_id() -> AccountId {
    "operator.near".parse().expect("valid account")
}

fn public_key() -> PublicKey {
    PublicKey::from(
        PUBLIC_KEY
            .parse::<near_api::PublicKey>()
            .expect("valid key"),
    )
}

async fn steps() -> Vec<(String, PlannedTransaction)> {
    crate::dispatch::plan::build(
        &offline_client(),
        &alpha_market(),
        &public_key(),
        &signer_id(),
    )
    .await
    .expect("the alpha fixture should plan")
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

/// The deployment `deploy.sh` performs, in the order it performs it.
///
/// The order is a safety property: `registry deploy` fails when the account
/// already exists, so governance must be created before the oracle names it as
/// owner, and both before the market points at the oracle.
#[tokio::test]
async fn plans_the_deploy_script_in_order() {
    let labelled = steps().await;
    let sequence: Vec<_> = labelled
        .iter()
        .map(|(label, transaction)| {
            let method = match transaction.actions.as_slice() {
                [near_api::types::transaction::actions::Action::FunctionCall(call)] => {
                    call.method_name.as_str()
                }
                other => panic!("expected one function call, got {other:?}"),
            };
            (transaction.receiver_id.as_str(), method, label.as_str())
        })
        .collect();

    // The three registry deploys share one method (`deploy_market`), so the
    // receiver alone cannot tell them apart either — the label is what
    // identifies each, and is asserted here for exactly that reason.
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

/// The headline criterion: what `apply` sends is exactly what `plan` showed.
///
/// Compared after conversion into the file, because that conversion
/// canonicalizes JSON args — see the module docs on `spec::plan`. Asserting on
/// the pre-canonical bytes would be asserting on `serde_json`'s key ordering,
/// not on anything a contract can observe.
#[tokio::test]
async fn round_trips_through_json() {
    let file = plan_file(steps().await);

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

/// Args a human can edit. Every step of a market deploy takes JSON, so every
/// step must carry JSON — a plan of base64 blobs cannot be edited, which is the
/// point of the artifact.
#[tokio::test]
async fn json_args_stay_json() {
    let file = plan_file(steps().await);

    for step in &file.steps {
        for call in &step.function_calls {
            assert!(
                matches!(call.args, PlanArgs::Json(_)),
                "`{}` in `{}` should carry editable JSON, got {:?}",
                call.method_name,
                step.label,
                call.args
            );
        }
    }
}

/// Borsh args cannot be JSON, and must survive verbatim.
///
/// `registry add_version` is the real case. Probing (rather than a hardcoded
/// method list) is what decides this, so the bytes here are a borsh payload
/// shape — a length-prefixed string — not merely "not JSON".
#[test]
fn borsh_args_stay_base64_and_survive() {
    let borsh: Vec<u8> = [
        &6u32.to_le_bytes()[..],
        b"v1.3.0",
        &[0u8, 1, 2, 3, 250, 251, 252],
    ]
    .concat();

    let plan = OperationPlan {
        steps: vec![PlannedTransaction {
            signer_account_id: signer_id().into(),
            receiver_id: "registry.near".parse().expect("valid account"),
            actions: vec![near_api::types::transaction::actions::Action::FunctionCall(
                Box::new(near_api::types::transaction::actions::FunctionCallAction {
                    method_name: "add_version".to_owned(),
                    args: borsh.clone(),
                    gas: templar_gateway_types::NearGas::from_tgas(300),
                    deposit: near_api::types::NearToken::from_near(1),
                }),
            )],
            continue_on_failure: false,
        }],
    };

    let file = plan_file(vec![("add a version".to_owned(), plan.steps[0].clone())]);
    assert!(
        matches!(file.steps[0].function_calls[0].args, PlanArgs::Base64(_)),
        "borsh args must not be mistaken for JSON: {:?}",
        file.steps[0].function_calls[0].args
    );

    let text = serde_json::to_string(&file).expect("serialize");
    let parsed: PlanFile = serde_json::from_str(&text).expect("deserialize");
    let restored = parsed.into_operation_plan().expect("convert back");

    assert_eq!(restored, plan, "opaque args must survive byte-for-byte");
}

/// Editing is the feature, so an edit is reported rather than refused — and
/// reported precisely enough to confirm it was the only one.
#[tokio::test]
async fn an_edited_plan_names_the_steps_that_changed() {
    let mut file = plan_file(steps().await);
    assert!(
        file.drift().expect("digest").is_clean(),
        "a freshly generated plan is unmodified"
    );

    file.steps[2].function_calls[0].gas += 1;
    let drift = file.drift().expect("digest");

    assert_eq!(drift.changed, vec![2], "only step 2 was touched");
    assert_eq!((drift.added, drift.removed), (0, 0));
    assert!(
        drift.describe().contains("1 step(s) differ (#2)"),
        "the operator must be told which step: {}",
        drift.describe()
    );
}

/// Removing a step is an edit too, and must not read as unmodified.
#[tokio::test]
async fn a_removed_step_is_reported() {
    let mut file = plan_file(steps().await);
    file.steps.pop();

    let drift = file.drift().expect("digest");
    assert_eq!(drift.removed, 1);
    assert!(!drift.is_clean());
}
