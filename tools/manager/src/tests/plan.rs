//! Dispatch-level tests: drive the CLI arg → spec → gateway `plan` path and
//! assert on the resulting transaction actions (receiver, method, args, deposit,
//! gas). Planning is offline for these writes — no signer or network is needed —
//! so a bug in the arg→spec mapping surfaces as a wrong on-chain call here, which
//! the param-serialization tests cannot catch.

use clap::Parser;
use near_account_id::AccountId;
use near_api::types::transaction::actions::Action;
use serde_json::Value;
use templar_gateway_client::{Client, Network, NetworkConfigBuilder};
use templar_gateway_core::{GatewayContext, OperationPlan, PlanWrite};
use templar_gateway_methods_dispatch::Dispatch;
use templar_gateway_types::{
    common::{WriteOperationResult, WriteRequest},
    ManagedAccountId, MethodSpec,
};

use super::{parse_governance, CREDS};
use crate::cli::{Cli, Command};
use crate::commands::proxy_oracle::ProxyOracleGovernanceNs;
use crate::commands::{FtNs, RedstoneNs, StorageNs};

const TGAS: u64 = 1_000_000_000_000;

fn offline_client() -> Client {
    Client::builder(NetworkConfigBuilder::new(Network::Testnet).build())
        .build()
        .expect("build offline client")
}

fn signer() -> ManagedAccountId {
    ManagedAccountId::from("signer.testnet".parse::<AccountId>().unwrap())
}

/// Plan `body` against a source-free [`GatewayContext`]. The `oracle.*` updates each
/// fetch a payload from an in-process source before building any step, so none of them
/// plans offline; their arg→spec mapping is covered by the CLI tests instead.
async fn plan<S>(body: S) -> OperationPlan
where
    S: MethodSpec<Output = WriteOperationResult>,
    Dispatch: PlanWrite<S, GatewayContext>,
{
    offline_client()
        .via::<Dispatch>()
        .plan_request(WriteRequest {
            signer_account_id: signer(),
            idempotency_key: None,
            body,
        })
        .await
        .expect("offline plan")
}

struct Call {
    receiver_id: String,
    method_name: String,
    args: Value,
    deposit: u128,
    gas: u64,
}

fn single_call(plan: &OperationPlan) -> Call {
    assert_eq!(plan.steps.len(), 1, "expected a single-step plan");
    let step = &plan.steps[0];
    assert_eq!(step.actions.len(), 1, "expected a single action");
    match &step.actions[0] {
        Action::FunctionCall(fc) => Call {
            receiver_id: step.receiver_id.to_string(),
            method_name: fc.method_name.clone(),
            args: serde_json::from_slice(&fc.args).expect("action args are JSON"),
            deposit: fc.deposit.as_yoctonear(),
            gas: fc.gas.as_gas(),
        },
        other => panic!("expected a FunctionCall action, got {other:?}"),
    }
}

#[tokio::test]
async fn ft_transfer_plans_ft_transfer_action() {
    let cli = Cli::try_parse_from(
        [
            "tmplrmgr",
            "ft",
            "transfer",
            "--contract-id",
            "usdt.testnet",
            "--receiver-id",
            "beneficiary.testnet",
            "--amount",
            "1000",
            "--memo",
            "recovered",
        ]
        .into_iter()
        .chain(CREDS),
    )
    .expect("ft transfer should parse");
    let body = match cli.command {
        Command::Ft {
            command: FtNs::Transfer(a),
        } => a.into_spec(),
        _ => panic!("expected ft transfer"),
    };

    let call = single_call(&plan(body).await);
    assert_eq!(call.receiver_id, "usdt.testnet");
    assert_eq!(call.method_name, "ft_transfer");
    assert_eq!(call.args["receiver_id"], "beneficiary.testnet");
    assert_eq!(call.args["amount"], "1000");
    assert_eq!(call.args["memo"], "recovered");
    assert_eq!(call.deposit, 1);
    assert_eq!(call.gas, 100 * TGAS);
}

#[tokio::test]
async fn storage_deposit_plans_storage_deposit_action() {
    let cli = Cli::try_parse_from(
        [
            "tmplrmgr",
            "storage",
            "deposit",
            "--contract-id",
            "usdt.testnet",
            "--beneficiary-id",
            "alice.testnet",
            "--deposit",
            "0.00125 NEAR",
        ]
        .into_iter()
        .chain(CREDS),
    )
    .expect("storage deposit should parse");
    let body = match cli.command {
        Command::Storage {
            command: StorageNs::Deposit(a),
        } => a.into_spec(),
        _ => panic!("expected storage deposit"),
    };

    let call = single_call(&plan(body).await);
    assert_eq!(call.receiver_id, "usdt.testnet");
    assert_eq!(call.method_name, "storage_deposit");
    assert_eq!(call.args["account_id"], "alice.testnet");
    assert_eq!(call.args["registration_only"], false);
    assert_eq!(call.deposit, 1_250_000_000_000_000_000_000);
}

#[tokio::test]
async fn storage_unregister_plans_storage_unregister_action() {
    let cli = Cli::try_parse_from(
        [
            "tmplrmgr",
            "storage",
            "unregister",
            "--contract-id",
            "usdt.testnet",
            "--force",
        ]
        .into_iter()
        .chain(CREDS),
    )
    .expect("storage unregister should parse");
    let body = match cli.command {
        Command::Storage {
            command: StorageNs::Unregister(a),
        } => a.into_spec(),
        _ => panic!("expected storage unregister"),
    };

    let call = single_call(&plan(body).await);
    assert_eq!(call.receiver_id, "usdt.testnet");
    assert_eq!(call.method_name, "storage_unregister");
    assert_eq!(call.args["force"], true);
    assert_eq!(call.deposit, 1);
}

#[tokio::test]
async fn governance_cancel_proposal_plans_cancel_action() {
    let body = match parse_governance(
        [
            "cancel-proposal",
            "--governance-id",
            "proxy.registry.testnet",
            "--id",
            "3",
        ]
        .into_iter()
        .chain(CREDS),
    ) {
        ProxyOracleGovernanceNs::CancelProposal(a) => {
            a.cancel("proxy.registry.testnet".parse().expect("valid account id"))
        }
        _ => panic!("expected cancel-proposal"),
    };

    let call = single_call(&plan(body).await);
    assert_eq!(call.receiver_id, "proxy.registry.testnet");
    assert_eq!(call.method_name, "cancel_proposal");
    assert_eq!(call.args["id"], 3);
    assert_eq!(call.deposit, 1);
    assert_eq!(call.gas, 300 * TGAS);
}

#[tokio::test]
async fn redstone_set_role_plans_set_role_action() {
    let cli = Cli::try_parse_from(
        [
            "tmplrmgr",
            "redstone",
            "set-role",
            "--oracle-id",
            "redstone.testnet",
            "--account-id",
            "updater.testnet",
            "--role",
            "trusted-updater",
        ]
        .into_iter()
        .chain(CREDS),
    )
    .expect("redstone set-role should parse");
    let body = match cli.command {
        Command::Redstone {
            command: RedstoneNs::SetRole(a),
        } => a.into_spec(),
        _ => panic!("expected redstone set-role"),
    };

    let call = single_call(&plan(body).await);
    assert_eq!(call.receiver_id, "redstone.testnet");
    assert_eq!(call.method_name, "set_role");
    assert_eq!(call.args["account_id"], "updater.testnet");
    // `--revoke` absent ⇒ grant the role.
    assert_eq!(call.args["set"], true);
    assert_eq!(call.deposit, 1);
}
