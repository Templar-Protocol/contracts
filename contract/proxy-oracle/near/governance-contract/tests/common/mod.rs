//! Minimal `near_api` helpers for the sandbox upgrade tests, mirroring the proxy-oracle test common.
//! Every harness account shares the same well-known test key, so one [`signer`] signs for any of
//! them; reads and writes pin the shared [`TEST_FINALITY_POLICY`] for deterministic finality.
#![allow(dead_code, clippy::unwrap_used, clippy::expect_used)]

use anyhow::{ensure, Context, Result};
use near_api::types::transaction::result::{ExecutionSuccess, TransactionResult};
use near_api::types::{AccountId, PublicKey};
use near_api::{Account, Contract, NetworkConfig};
use near_sdk::json_types::Base58CryptoHash;
use near_sdk::serde::{de::DeserializeOwned, Serialize};
use templar_common::{oracle::pyth::PriceIdentifier, Nanoseconds};
use templar_proxy_oracle_kernel::proxy::{
    circuit_breaker::CircuitBreakerSet, FreshnessFilter, Proxy,
};
use templar_proxy_oracle_near_common::{input::Source, request::OracleRequest};
use templar_proxy_oracle_near_governance_common::{Operation, Proposal, ReflexiveOperation};

pub use templar_gateway_testing::test_signer as signer;
use templar_gateway_testing::{SandboxHarness, TEST_FINALITY_POLICY};

/// Dispatch a view call and deserialize the result.
pub async fn view<T: DeserializeOwned + Send + Sync>(
    network: &NetworkConfig,
    contract_id: &AccountId,
    method: &str,
    args: impl Serialize,
) -> Result<T> {
    Ok(Contract(contract_id.clone())
        .call_function(method, args)
        .read_only::<T>()
        .at(TEST_FINALITY_POLICY.query_reference())
        .fetch_from(network)
        .await?
        .data)
}

/// Signed by `account_id`, which pays: a global deploy costs ten times a normal one, permanently.
pub async fn deploy_global_contract(
    network: &NetworkConfig,
    account_id: &AccountId,
    code: Vec<u8>,
) -> Result<Base58CryptoHash> {
    let hash = near_api::types::CryptoHash::hash(&code).0;
    Contract::deploy_global_contract_code(code)
        .as_hash()
        .with_signer(account_id.clone(), signer())
        .wait_until(TEST_FINALITY_POLICY.transaction_status())
        .send_to(network)
        .await?
        .assert_success();
    Ok(Base58CryptoHash::from(hash))
}

/// Deploy `code` to `account` via its full-access key, atomically calling `method` (`new` to init,
/// `migrate` to bootstrap-upgrade).
pub async fn deploy_with_init(
    network: &NetworkConfig,
    account: &AccountId,
    code: Vec<u8>,
    method: &str,
    args: near_sdk::serde_json::Value,
) -> Result<()> {
    Contract::deploy(account.clone())
        .use_code(code)
        .with_init_call(method, args)?
        .max_gas()
        .with_signer(signer())
        .wait_until(TEST_FINALITY_POLICY.transaction_status())
        .send_to(network)
        .await?
        .assert_success();
    Ok(())
}

/// What the gateway attaches (`proxy_oracle_governance_impl`), and so what these tests attach by
/// default — well under the protocol's own prepaid-gas ceiling. It is this budget, not a protocol
/// limit, that an `admin_upgrade` forwarding `GAS_FOR_ADMIN_UPGRADE` has to fit inside.
pub const GATEWAY_GAS: near_sdk::Gas = near_sdk::Gas::from_tgas(300);

/// Submit a mutating call to `contract_id` signed as `signer_id` (all harness accounts share the
/// test key), attaching `deposit` and [`GATEWAY_GAS`].
pub async fn call(
    network: &NetworkConfig,
    contract_id: &AccountId,
    method: &str,
    args: impl Serialize,
    signer_id: &AccountId,
    deposit: near_sdk::NearToken,
) -> Result<ExecutionSuccess> {
    let args = near_sdk::serde_json::to_vec(&args)?;
    call_raw(
        network,
        contract_id,
        method,
        args,
        signer_id,
        deposit,
        GATEWAY_GAS,
    )
    .await
}

pub async fn call_borsh(
    network: &NetworkConfig,
    contract_id: &AccountId,
    method: &str,
    args: impl near_sdk::borsh::BorshSerialize,
    signer_id: &AccountId,
    deposit: near_sdk::NearToken,
) -> Result<ExecutionSuccess> {
    let args = near_sdk::borsh::to_vec(&args)?;
    call_raw(
        network,
        contract_id,
        method,
        args,
        signer_id,
        deposit,
        GATEWAY_GAS,
    )
    .await
}

/// Submit the call without asserting anything about its outcome — for paths expected to revert.
async fn send(
    network: &NetworkConfig,
    contract_id: &AccountId,
    method: &str,
    args: Vec<u8>,
    signer_id: &AccountId,
    deposit: near_sdk::NearToken,
    gas: near_sdk::Gas,
) -> Result<TransactionResult> {
    Ok(Contract(contract_id.clone())
        .call_function_raw(method, args)
        .transaction()
        .deposit(deposit)
        .gas(gas)
        .with_signer(signer_id.clone(), signer())
        .wait_until(TEST_FINALITY_POLICY.transaction_status())
        .send_to(network)
        .await?)
}

async fn call_raw(
    network: &NetworkConfig,
    contract_id: &AccountId,
    method: &str,
    args: Vec<u8>,
    signer_id: &AccountId,
    deposit: near_sdk::NearToken,
    gas: near_sdk::Gas,
) -> Result<ExecutionSuccess> {
    let outcome = send(network, contract_id, method, args, signer_id, deposit, gas)
        .await?
        .assert_success();

    // Governance dispatches its effects on detached promises, so a failed target call still leaves
    // the top-level status successful.
    let failures = outcome.receipt_failures();
    assert!(
        failures.is_empty(),
        "{method} left failed receipts: {failures:#?}"
    );
    Ok(outcome)
}

pub use templar_proxy_oracle_near_governance_common::CreateProposalArgs;

pub const ONE_YOCTO: near_sdk::NearToken = near_sdk::NearToken::from_yoctonear(1);

/// A `SelfUpgrade` with nothing for `migrate` to do — the state version is already current.
pub fn self_upgrade(code: templar_common::upgrade::UpgradeSource) -> Operation {
    Operation::Reflexive(ReflexiveOperation::SelfUpgrade {
        code,
        migrate_args: near_sdk::json_types::Base64VecU8(Vec::new()),
    })
}

#[derive(Serialize)]
#[serde(crate = "near_sdk::serde")]
pub struct ProposalIdArgs {
    pub id: u32,
}

/// The account's deployed code hash, as a stable string for equality checks (a change proves the
/// code was actually replaced — i.e. the upgrade took effect and didn't revert).
pub async fn code_hash(network: &NetworkConfig, id: &AccountId) -> Result<String> {
    // `contract_state` is `ContractState::LocalHash(<hash>)` for a normally-deployed contract.
    Ok(format!(
        "{:?}",
        Account(id.clone())
            .view()
            .at(TEST_FINALITY_POLICY.query_reference())
            .fetch_from(network)
            .await?
            .data
            .contract_state
    ))
}

/// Delete every access key on `account_id`, signed by one of those keys — the last thing it can do.
/// Nothing off-chain can deploy over the account afterwards; only its own code can.
pub async fn revoke_all_access_keys(
    harness: &SandboxHarness,
    account_id: &AccountId,
) -> Result<()> {
    let keys = harness
        .view_access_keys(account_id)
        .await?
        .into_iter()
        .map(|(public_key, _)| public_key.parse::<PublicKey>())
        .collect::<Result<Vec<_>, _>>()?;
    ensure!(!keys.is_empty(), "{account_id} already has no access keys");

    Account(account_id.clone())
        .delete_keys(keys)
        .with_signer(signer())
        .wait_until(TEST_FINALITY_POLICY.transaction_status())
        .send_to(&harness.network)
        .await?
        .assert_success();

    ensure!(
        harness.view_access_keys(account_id).await?.is_empty(),
        "{account_id} should have no access keys left"
    );
    Ok(())
}

/// The feed the [`Governed`] fixture seeds a proxy under.
pub const PRICE_ID: PriceIdentifier = PriceIdentifier([0xaa; 32]);

#[derive(Serialize)]
#[serde(crate = "near_sdk::serde")]
pub struct PriceIdArgs {
    pub id: PriceIdentifier,
}

#[derive(Clone, Copy)]
pub enum Encoding {
    Json,
    Borsh,
}

/// An oracle owned by a governance contract, with a proxy already in place.
pub struct Governed {
    pub network: NetworkConfig,
    pub oracle: AccountId,
    pub governance: AccountId,
    pub admin: AccountId,
    next_id: u32,
}

impl Governed {
    /// Create a proposal as the admin, returning its id alongside the creation outcome. Every TTL is
    /// zero (`deploy_governance_contract` installs a uniform zero policy), so it matures immediately.
    pub async fn propose(
        &mut self,
        operation: Operation,
        encoding: Encoding,
    ) -> Result<(u32, ExecutionSuccess)> {
        let id = self.next_id;
        self.next_id += 1;
        let args = CreateProposalArgs {
            id,
            operation,
            requested_ttl: Nanoseconds::zero(),
        };

        let outcome = match encoding {
            Encoding::Json => {
                call(
                    &self.network,
                    &self.governance,
                    "create_proposal",
                    args,
                    &self.admin,
                    ONE_YOCTO,
                )
                .await?
            }
            Encoding::Borsh => {
                call_borsh(
                    &self.network,
                    &self.governance,
                    "create_proposal_borsh",
                    args,
                    &self.admin,
                    ONE_YOCTO,
                )
                .await?
            }
        };
        Ok((id, outcome))
    }

    pub async fn execute(&self, id: u32) -> Result<ExecutionSuccess> {
        call(
            &self.network,
            &self.governance,
            "execute_proposal",
            ProposalIdArgs { id },
            &self.admin,
            ONE_YOCTO,
        )
        .await
    }

    /// [`execute`](Self::execute) attaching `gas` instead of [`GATEWAY_GAS`], and without the
    /// success assertion — for budgets a proposal may or may not fit inside.
    pub async fn try_execute_with_gas(
        &self,
        id: u32,
        gas: near_sdk::Gas,
    ) -> Result<TransactionResult> {
        send(
            &self.network,
            &self.governance,
            "execute_proposal",
            near_sdk::serde_json::to_vec(&ProposalIdArgs { id })?,
            &self.admin,
            ONE_YOCTO,
            gas,
        )
        .await
    }

    pub async fn proposal(&self, id: u32) -> Result<Option<Proposal<Operation>>> {
        view(
            &self.network,
            &self.governance,
            "get_proposal",
            ProposalIdArgs { id },
        )
        .await
    }

    /// Run `operation` through the full create → execute path as the admin.
    pub async fn govern(&mut self, operation: Operation) -> Result<ExecutionSuccess> {
        self.govern_with(operation, Encoding::Json).await
    }

    pub async fn govern_borsh(&mut self, operation: Operation) -> Result<ExecutionSuccess> {
        self.govern_with(operation, Encoding::Borsh).await
    }

    pub async fn govern_with(
        &mut self,
        operation: Operation,
        encoding: Encoding,
    ) -> Result<ExecutionSuccess> {
        let (id, _) = self.propose(operation, encoding).await?;
        self.execute(id).await
    }

    pub async fn proxy(&self) -> Result<Option<Proxy<Source>>> {
        view(
            &self.network,
            &self.oracle,
            "get_proxy",
            PriceIdArgs { id: PRICE_ID },
        )
        .await
    }

    pub async fn breakers(&self) -> Result<CircuitBreakerSet> {
        view::<Option<CircuitBreakerSet>>(
            &self.network,
            &self.oracle,
            "get_proxy_circuit_breaker_set",
            PriceIdArgs { id: PRICE_ID },
        )
        .await?
        .context("the seeded proxy is gone")
    }
}

pub fn proxy() -> Proxy<Source> {
    Proxy::median_low(
        [OracleRequest::pyth(
            "pyth.near".parse().expect("literal account id is valid"),
            PriceIdentifier([0xbb; 32]),
        )
        .into()],
        FreshnessFilter::empty(),
    )
}

/// Deploy the oracle with a proxy in place, then hand it to a governance contract. The proxy is
/// seeded while the oracle still owns itself — one direct call instead of a governance round-trip.
pub async fn governed(harness: &SandboxHarness) -> Result<Governed> {
    let oracle = harness.deploy_proxy_oracle().await?;
    let admin = harness.create_user("admin").await?.0;
    harness
        .admin_set_proxy(oracle.clone(), PRICE_ID, Some(proxy()))
        .await?;
    let governance = harness
        .deploy_governance_contract(oracle.clone(), admin.clone())
        .await?;

    Ok(Governed {
        network: harness.network.clone(),
        oracle,
        governance,
        admin,
        // `deploy_governance_contract` consumes id 0 for the ownership handover.
        next_id: 1,
    })
}
