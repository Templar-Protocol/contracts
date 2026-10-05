//! `registry upgrade`: the self-signed deploy-and-`migrate` batch for a pre-2.0.0 registry. It
//! cannot be rolled back, so every check gates submission and none can be skipped.

use std::{collections::HashMap, num::NonZeroU32};

use anyhow::{Context as _, Result};
use futures::{StreamExt as _, TryStreamExt as _};
use near_account_id::AccountId;
use near_api::types::transaction::actions::{AccessKeyPermission, Action};
use near_primitives::{account::AccountContract, hash::CryptoHash};
use near_token::NearToken;
use serde::Serialize;
use templar_contract_artifacts::{fetch, ArtifactId, ArtifactRelease};
use templar_gateway_client::{collect_paginated, Client};
use templar_gateway_core::{GatewayError, OperationPlan};
use templar_gateway_methods_dispatch::registry_release;
use templar_gateway_methods_spec::{
    account, chain,
    contract::{self, GetStateVersionResult},
    owner,
    registry::{self, Migration},
};
use templar_gateway_types::{
    common::{Pagination, WriteOperationResult, WriteRequest},
    ManagedAccountId, NearGas, OperationStatus, ProtocolLimits, RegistryVersion,
};

use crate::commands::registry::{Upgrade, STORAGE_AMOUNT_PER_BYTE};
use crate::commands::signer::{Authorization, Mode, PrintFormat};
use crate::context::{check_operation_status, print_json, print_plan, CliContext};
use crate::dispatch::{
    patch_state::{fetch_state, snapshot_final, StateSnapshot},
    sandbox_replay::{
        build_local_client, reset_account_metadata, setup_account, stage_local_code, start_sandbox,
    },
};
use crate::report::Reporter;
use crate::spec::check::{gate_unskippable, Check, Report, Status};

/// Under the 4 MB receipt proof limit with room for mainnet's deeper trie, and about the most whose
/// migration burns within half of the gas `migrate` attaches.
const PRE_GLOBAL_CONTRACTS_STATE_BUDGET: u64 = 3_000_000;

/// Balance kept free after staking storage for the new code, for the transaction fee.
const BALANCE_HEADROOM: NearToken = NearToken::from_near(1);

/// What a 1.1.0+ registry's root sheds with its retired collection.
const RETIRED_COLLECTION_ALLOWANCE: u64 = 64;

/// What any migration may add for its state version record.
const VERSION_RECORD_ALLOWANCE: u64 = 256;

#[allow(
    clippy::unwrap_used,
    reason = "compile-time const; a zero literal would fail to compile"
)]
const PAGE: NonZeroU32 = NonZeroU32::new(100).unwrap();

const CONCURRENT_READS: usize = 8;

/// near-sdk's `IterableMap` keeps key `i` at `prefix + "v" + i` and its value, first, at
/// `sha256(prefix + "m" + borsh(key))`; legacy registries use prefixes `v` and `r`.
const VERSION_KEYS_PREFIX: &[u8] = b"vv";
const REGISTRY_KEYS_PREFIX: &[u8] = b"rv";
const REGISTRY_VALUES_PREFIX: &[u8] = b"rm";

/// `RegistryEntry::Reserved`'s borsh tag.
const RESERVED_TAG: u8 = 0;

/// Rounds of the on-chain verification, for an RPC backend still behind the transaction.
const VERIFY_ATTEMPTS: u32 = 5;
const VERIFY_RETRY_DELAY: std::time::Duration = std::time::Duration::from_secs(3);

/// Everything a registry holds that a migration must carry over unchanged.
#[derive(Debug, Clone, PartialEq, Eq)]
struct RegistryContents {
    owner: Option<AccountId>,
    versions: Vec<(String, Option<templar_gateway_types::CryptoHash>)>,
    deployments: Vec<AccountId>,
}

#[derive(Serialize)]
struct Subject<'a> {
    registry_id: &'a AccountId,
    #[serde(skip_serializing_if = "Option::is_none")]
    operation: Option<&'a WriteOperationResult>,
}

/// What an upgraded registry must look like, beyond holding what it held before.
struct Expected {
    code_hash: CryptoHash,
    /// Storage with the code swapped and the state untouched; [`storage_status`] bounds the rest.
    storage_usage: u64,
    growth: u64,
    shrink: u64,
}

/// Where a run that passes every check ends up.
enum Sink {
    Print(PrintFormat),
    Submit(Client),
}

impl Expected {
    /// The same transaction on byte-identical state, so mainnet must land on what the replay
    /// measured.
    fn exactly(&self, storage_usage: u64) -> Self {
        Self {
            code_hash: self.code_hash,
            storage_usage,
            growth: 0,
            shrink: 0,
        }
    }
}

/// What the preflight establishes before anything runs in the sandbox.
struct Prepared {
    snapshot: StateSnapshot,
    body: registry::Upgrade,
    expected: Expected,
    replay_key: near_api::types::PublicKey,
    limits: ProtocolLimits,
}

pub(super) async fn upgrade(ctx: CliContext, args: Upgrade) -> Result<()> {
    let registry_id = args.registry_id().clone();
    let authorization = Authorization::try_from(&args.signer)?;
    anyhow::ensure!(
        !matches!(authorization.mode(), Mode::Plan(PrintFormat::Sputnik)),
        "--print sputnik cannot carry this upgrade: it deploys code, which a DAO proposal cannot"
    );
    let mut reporter = ctx.reporter(&[]);

    reporter.phase(&format!("preflight for {registry_id}"));
    reporter.record(Check::new(
        "upgrade.network",
        crate::spec::network_status(ctx.network(), &registry_id),
    ));
    reporter.record(Check::new(
        "upgrade.signer",
        signer_status(&authorization.account_id().0, &registry_id),
    ));
    if reporter.has_failures() {
        return finish(&registry_id, reporter);
    }

    let (sink, signing_key) = match sink(&ctx, authorization).await {
        Ok(resolved) => resolved,
        Err(error) => {
            reporter.record(Check::new(
                "upgrade.signing_key",
                Status::failed(format!("{error:#}")),
            ));
            return finish(&registry_id, reporter);
        }
    };

    let prepared = prepare(
        &ctx,
        &registry_id,
        args.release(),
        signing_key.as_ref(),
        &mut reporter,
    )
    .await;
    let prepared = match prepared {
        Some(prepared) if !reporter.has_failures() => prepared,
        _ => return finish(&registry_id, reporter),
    };

    let request = WriteRequest {
        signer_account_id: ManagedAccountId(registry_id.clone()),
        idempotency_key: None,
        body: prepared.body.clone(),
    };
    let (mainnet_plan, before) = match plan_and_read(&ctx, &request).await {
        Ok(planned) => planned,
        Err(error) => {
            reporter.record(Check::new(
                "upgrade.plan",
                Status::failed(format!("{error:#}")),
            ));
            return finish(&registry_id, reporter);
        }
    };

    reporter.record(Check::new(
        "upgrade.no_reserved",
        reserved_status(&prepared.snapshot, before.deployments.len()),
    ));
    if reporter.has_failures() {
        return finish(&registry_id, reporter);
    }

    reporter.phase("sandbox replay of the exact transaction");
    let replayed_storage =
        replay_storage(&prepared, &request, &mainnet_plan, &before, &mut reporter).await;
    let Some(replayed_storage) = replayed_storage.filter(|_| !reporter.has_failures()) else {
        return finish(&registry_id, reporter);
    };

    reporter.phase("drift since the snapshot");
    reporter.record(Check::new(
        "upgrade.drift",
        drift_status(&ctx, &registry_id, &prepared.snapshot, &prepared.limits).await,
    ));
    if reporter.has_failures() {
        return finish(&registry_id, reporter);
    }

    match sink {
        Sink::Print(format) => {
            reporter.digest();
            print_plan(format, mainnet_plan)
        }
        Sink::Submit(client) => {
            submit(
                &ctx,
                &client,
                request,
                &prepared.expected.exactly(replayed_storage),
                &mainnet_plan,
                &before,
                reporter,
            )
            .await
        }
    }
}

/// A printed plan names the key that will sign it; a submission resolves its signer here.
async fn sink(
    ctx: &CliContext,
    authorization: Authorization,
) -> Result<(Sink, Option<near_api::types::PublicKey>)> {
    if let Mode::Plan(format) = *authorization.mode() {
        return Ok((
            Sink::Print(format),
            authorization.public_key().ok().map(|key| key.0),
        ));
    }
    let (_, client, key) = ctx.signing_client_and_key(authorization).await?;
    Ok((Sink::Submit(client), Some(key)))
}

/// The transaction mainnet would run now, and what the registry holds before it does.
async fn plan_and_read(
    ctx: &CliContext,
    request: &WriteRequest<registry::Upgrade>,
) -> Result<(OperationPlan, RegistryContents)> {
    let final_client = ctx.final_client()?;
    futures::try_join!(
        async { Ok(ctx.client.plan_request(request.clone()).await?) },
        read_contents(&final_client, &request.body.registry_id),
    )
}

/// Sign and send the batch, then require the chain to show what the replay showed.
async fn submit(
    ctx: &CliContext,
    client: &Client,
    request: WriteRequest<registry::Upgrade>,
    expected: &Expected,
    replayed_plan: &OperationPlan,
    before: &RegistryContents,
    mut reporter: Reporter,
) -> Result<()> {
    let registry_id = request.body.registry_id.clone();
    // Planned once more by the client that sends, so a failure here provably sent nothing.
    reporter.record(Check::new(
        "upgrade.submit_plan",
        match client.plan_request(request.clone()).await {
            Ok(plan) if plan.steps == replayed_plan.steps => {
                Status::passed("the signer plans the transaction the sandbox replayed")
            }
            Ok(_) => Status::failed("the signer plans a different transaction than was replayed"),
            Err(error) => Status::failed(format!("plan with the signer: {error}")),
        },
    ));
    if reporter.has_failures() {
        return finish(&registry_id, reporter);
    }

    let submitted = client
        .execute_as(request.signer_account_id, request.body)
        .await;
    // A broadcast whose outcome never came back, or is not yet terminal, may still be landing, so
    // reading the registry now would misreport it either way.
    let unknown = match &submitted {
        Ok(output) => matches!(
            output.operation.status,
            OperationStatus::Pending | OperationStatus::InProgress
        )
        .then(|| format!("{:?}", output.operation.status)),
        Err(error) => Some(error.to_string()),
    };
    if let Some(reason) = unknown {
        if let Ok(output) = &submitted {
            ctx.report_tx(output);
        }
        reporter.record(Check::new(
            "upgrade.submit",
            Status::failed(format!(
                "outcome unknown: {reason}. The transaction may still land; read {registry_id}'s \
                 code hash and state version before doing anything else"
            )),
        ));
        reporter.digest();
        print_report(&registry_id, reporter.checks(), submitted.as_ref().ok())?;
        anyhow::bail!("the upgrade of {registry_id} was submitted with an unknown outcome");
    }
    let output = submitted?;
    ctx.report_tx(&output);

    reporter.phase("the registry on chain");
    // The signing client reads the state its own transaction executed into; finalized state may
    // still predate it.
    let mut verified = Vec::new();
    for attempt in 1..=VERIFY_ATTEMPTS {
        (verified, _) = verify(client, &registry_id, expected, before, "upgrade.verify").await;
        let settled = output.operation.status != OperationStatus::Succeeded
            || !verified.iter().any(|check| check.status.is_failure());
        if settled || attempt == VERIFY_ATTEMPTS {
            break;
        }
        tokio::time::sleep(VERIFY_RETRY_DELAY).await;
    }
    reporter.extend(verified);
    reporter.digest();
    let checks = reporter.into_checks();
    print_report(&registry_id, &checks, Some(&output))?;
    check_operation_status(&output)?;
    gate_unskippable(
        &checks,
        &format!("registry {registry_id}"),
        "the upgrade was submitted, but the registry does not verify as upgraded and intact",
    )
}

fn print_report(
    registry_id: &AccountId,
    checks: &[Check],
    operation: Option<&WriteOperationResult>,
) -> Result<()> {
    print_json(&Report {
        subject: Subject {
            registry_id,
            operation,
        },
        checks,
    })
}

/// Report a run that stopped before submitting, and fail it whatever the checks say.
fn finish(registry_id: &AccountId, mut reporter: Reporter) -> Result<()> {
    reporter.digest();
    let checks = reporter.into_checks();
    print_report(registry_id, &checks, None)?;
    gate_unskippable(
        &checks,
        &format!("registry {registry_id}"),
        "the upgrade was not submitted",
    )?;
    anyhow::bail!("the preflight for {registry_id} stopped early; the upgrade was not submitted")
}

/// The checks that need no sandbox. `None` when one failed so early that later ones cannot run.
async fn prepare(
    ctx: &CliContext,
    registry_id: &AccountId,
    release: Option<&str>,
    signing_key: Option<&near_api::types::PublicKey>,
    reporter: &mut Reporter,
) -> Option<Prepared> {
    let (status, target) = target_status(release);
    reporter.record(Check::new("upgrade.target_release", status));
    let target = target?;
    let Some(signing_key) = signing_key else {
        reporter.record(Check::new(
            "upgrade.signing_key",
            Status::failed(
                "pass --public-key with --print, naming the full-access key that will sign the plan",
            ),
        ));
        return None;
    };

    let metadata_version = async {
        ctx.client
            .read(contract::GetVersion {
                contract_id: registry_id.clone(),
            })
            .await
            .map(|version| version.version_string)
    };
    let snapshot = async {
        let limits = match ctx.client.read(chain::GetProtocolLimits).await {
            Ok(limits) => limits,
            Err(error) => {
                reporter.record(Check::new(
                    "upgrade.state_complete",
                    Status::failed(format!("read protocol limits: {error}")),
                ));
                return None;
            }
        };
        let snapshot = snapshot_final(
            ctx,
            registry_id,
            &limits,
            "upgrade.state_complete",
            reporter,
        )
        .await
        .ok()?;
        Some((snapshot, limits))
    };
    let (snapshot, metadata_version) = futures::join!(snapshot, metadata_version);
    let (snapshot, limits) = snapshot?;

    let (status, source) = source_status(&snapshot, &metadata_version);
    reporter.record(Check::new("upgrade.source_release", status));
    let source = source?;

    let (status, migration) = migration_status(source);
    reporter.record(Check::new("upgrade.migration", status));
    let migration = migration?;

    let wasm = match fetch::released_bytes(ArtifactId::Registry, target.version).await {
        Ok(wasm) => {
            reporter.record(Check::new(
                "upgrade.target_wasm",
                Status::passed(format!(
                    "{} bytes matching the catalogued sha256 {}",
                    wasm.len(),
                    target.sha256
                )),
            ));
            wasm
        }
        Err(error) => {
            reporter.record(Check::new(
                "upgrade.target_wasm",
                Status::failed(error.to_string()),
            ));
            return None;
        }
    };
    let expected = expected_after(&snapshot, migration, &wasm);

    let (status, replay_key) = signing_key_status(&snapshot, signing_key);
    reporter.record(Check::new("upgrade.signing_key", status));
    reporter.record(Check::new(
        "upgrade.balance",
        balance_status(snapshot.amount, &expected),
    ));
    reporter.record(Check::new(
        "upgrade.proof_budget",
        proof_budget_status(migration, &snapshot),
    ));

    Some(Prepared {
        expected,
        body: registry::Upgrade {
            registry_id: registry_id.clone(),
            wasm: templar_gateway_types::Base64Bytes(wasm),
            migration,
        },
        replay_key: replay_key?,
        snapshot,
        limits,
    })
}

fn signer_status(signer_id: &AccountId, registry_id: &AccountId) -> Status {
    if signer_id == registry_id {
        Status::passed(format!("signed by {registry_id}"))
    } else {
        Status::failed(format!(
            "--signer-id is {signer_id}, but only {registry_id} can deploy its own code and call \
             its private `migrate`"
        ))
    }
}

/// The catalogued registry release whose bytes the account runs, which must also be the version
/// its NEP-330 metadata reports: the migration is chosen from it, so both signals have to agree.
fn source_status(
    snapshot: &StateSnapshot,
    reported: &Result<String, GatewayError>,
) -> (Status, Option<RegistryVersion>) {
    let AccountContract::Local(_) = snapshot.contract else {
        return (
            Status::failed(format!(
                "the account runs {:?}, not locally deployed code",
                snapshot.contract
            )),
            None,
        );
    };
    let sha256 = CryptoHash::hash_bytes(&snapshot.code).0;
    let Some(release) = registry_release(&sha256) else {
        return (
            Status::failed(format!(
                "the account's code (sha256 {}) is not a catalogued registry release, so its \
                 state layout is unknown",
                hex::encode(sha256)
            )),
            None,
        );
    };
    let reported = match reported {
        Ok(reported) => reported,
        Err(error) => {
            return (
                Status::failed(format!("read NEP-330 metadata: {error}")),
                None,
            )
        }
    };
    if reported
        .parse::<RegistryVersion>()
        .map(<(u64, u64, u64)>::from)
        != Ok(release.into())
    {
        return (
            Status::failed(format!(
                "the code is registry {release} but its metadata reports {reported}"
            )),
            None,
        );
    }
    (Status::passed(format!("registry {release}")), Some(release))
}

fn release_version(release: &ArtifactRelease) -> Result<RegistryVersion, Status> {
    release
        .version
        .parse()
        .map_err(|error| Status::failed(format!("parse release {}: {error:?}", release.version)))
}

fn migration_status(source: RegistryVersion) -> (Status, Option<Migration>) {
    match Migration::for_version(source) {
        Some(migration) => (Status::passed(format!("{migration:?}")), Some(migration)),
        None => (
            Status::failed(format!(
                "registry {source} keeps a state version and upgrades through its owner-only \
                 `upgrade`, not this command"
            )),
            None,
        ),
    }
}

fn target_status(requested: Option<&str>) -> (Status, Option<&'static ArtifactRelease>) {
    let metadata = ArtifactId::Registry.metadata();
    let release = match requested {
        Some(version) => metadata.release(version),
        None => metadata.current(),
    };
    let Some(release) = release else {
        return (
            Status::failed(format!(
                "registry {} is not a catalogued release",
                requested.unwrap_or("(newest)")
            )),
            None,
        );
    };
    let version = match release_version(release) {
        Ok(version) => version,
        Err(status) => return (status, None),
    };
    if !version.supports_upgrade() {
        return (
            Status::failed(format!(
                "registry {version} predates versioned state, so it has no `migrate` to run"
            )),
            None,
        );
    }
    (Status::passed(format!("registry {version}")), Some(release))
}

/// The key that will sign must hold full access, since the batch deploys code.
fn signing_key_status(
    snapshot: &StateSnapshot,
    signing_key: &near_api::types::PublicKey,
) -> (Status, Option<near_api::types::PublicKey>) {
    let full_access = snapshot.access_keys.iter().any(|(held, access)| {
        held == signing_key && matches!(access.permission, AccessKeyPermission::FullAccess)
    });
    if full_access {
        (
            Status::passed(format!("{signing_key} holds full access")),
            Some(*signing_key),
        )
    } else {
        (
            Status::failed(format!(
                "{signing_key} is not a full-access key on the registry, so it cannot deploy code"
            )),
            None,
        )
    }
}

fn balance_status(amount: NearToken, expected: &Expected) -> Status {
    let after = expected.storage_usage + expected.growth;
    let required = STORAGE_AMOUNT_PER_BYTE
        .saturating_mul(u128::from(after))
        .saturating_add(BALANCE_HEADROOM);
    if amount >= required {
        Status::passed(format!(
            "{} covers {after} bytes of storage plus {} headroom",
            amount.exact_amount_display(),
            BALANCE_HEADROOM.exact_amount_display(),
        ))
    } else {
        Status::failed(format!(
            "{} is short of {} for {after} bytes of storage plus headroom",
            amount.exact_amount_display(),
            required.exact_amount_display(),
        ))
    }
}

fn proof_budget_status(migration: Migration, snapshot: &StateSnapshot) -> Status {
    let bytes: u64 = snapshot
        .entries
        .iter()
        .map(|entry| (entry.key.len() + entry.value.len()) as u64)
        .sum();
    match migration {
        Migration::WithGlobalContracts => Status::passed(format!(
            "this migration rewrites no stored code; {bytes} bytes of state stay where they are"
        )),
        Migration::PreGlobalContracts if bytes <= PRE_GLOBAL_CONTRACTS_STATE_BUDGET => {
            Status::passed(format!(
                "{bytes} bytes of state, within {PRE_GLOBAL_CONTRACTS_STATE_BUDGET}"
            ))
        }
        Migration::PreGlobalContracts => Status::failed(format!(
            "{bytes} bytes of state exceeds {PRE_GLOBAL_CONTRACTS_STATE_BUDGET}: the migration \
             rewrites every stored code blob in one receipt and would exceed the 4 MB proof \
             limit. Remove versions with `registry remove-version` first."
        )),
    }
}

fn expected_after(snapshot: &StateSnapshot, migration: Migration, wasm: &[u8]) -> Expected {
    let (growth, shrink) = match migration {
        Migration::PreGlobalContracts => {
            let versions = snapshot
                .entries
                .iter()
                .filter(|entry| is_vector_key(&entry.key, VERSION_KEYS_PREFIX))
                .count();
            (versions as u64, 0)
        }
        Migration::WithGlobalContracts => (0, RETIRED_COLLECTION_ALLOWANCE),
    };
    Expected {
        code_hash: CryptoHash::hash_bytes(wasm),
        storage_usage: snapshot.storage_usage - snapshot.code.len() as u64 + wasm.len() as u64,
        growth: growth + VERSION_RECORD_ALLOWANCE,
        shrink,
    }
}

/// A `Vector` element's key is its prefix and a `u32` index, which a hashed value key never is.
fn is_vector_key(key: &[u8], keys_prefix: &[u8]) -> bool {
    key.len() == keys_prefix.len() + size_of::<u32>() && key.starts_with(keys_prefix)
}

/// A name is `Reserved` from a deploy's first receipt until its finalize callback, which then runs
/// against the new code: pre-1.1.0's callback does not exist there, so it would stay reserved.
fn reserved_status(snapshot: &StateSnapshot, listed_deployments: usize) -> Status {
    let values: HashMap<&[u8], &[u8]> = snapshot
        .entries
        .iter()
        .map(|entry| (entry.key.as_slice(), entry.value.as_slice()))
        .collect();
    let names: Vec<&[u8]> = snapshot
        .entries
        .iter()
        .filter(|entry| is_vector_key(&entry.key, REGISTRY_KEYS_PREFIX))
        .map(|entry| entry.value.as_slice())
        .collect();
    let mut reserved = 0;
    for name in &names {
        let key = CryptoHash::hash_bytes(&[REGISTRY_VALUES_PREFIX, name].concat());
        match values.get(key.0.as_slice()).and_then(|value| value.first()) {
            Some(&RESERVED_TAG) => reserved += 1,
            Some(_) => {}
            None => {
                return Status::failed(
                    "a stored name has no entry where this release keeps one; the state layout \
                     is not the one this release is known to hold",
                )
            }
        }
    }
    let deployed = names.len() - reserved;
    if deployed != listed_deployments {
        Status::failed(format!(
            "{deployed} finished deployment(s) decoded but {listed_deployments} listed: a deploy \
             finished between the reads, so re-run, or the state layout is not this release's"
        ))
    } else if reserved == 0 {
        Status::passed(format!(
            "all {} name(s) are finished deployments",
            names.len()
        ))
    } else {
        Status::failed(format!(
            "{reserved} of {} name(s) are reserved by a deploy that has not finished; wait for it, \
             or clean it up, before upgrading",
            names.len()
        ))
    }
}

/// [`replay`], with an error that stops it recorded as a failed check.
async fn replay_storage(
    prepared: &Prepared,
    request: &WriteRequest<registry::Upgrade>,
    mainnet_plan: &OperationPlan,
    before: &RegistryContents,
    reporter: &mut Reporter,
) -> Option<u64> {
    match replay(prepared, request, mainnet_plan, before, reporter).await {
        Ok(storage) => storage,
        Err(error) => {
            reporter.record(Check::new(
                "upgrade.replay.sandbox",
                Status::failed(format!("{error:#}")),
            ));
            None
        }
    }
}

/// Rebuild the registry from the snapshot in a fresh sandbox, run the transaction mainnet would
/// run, and require that the registry comes out upgraded and holding exactly what it held.
async fn replay(
    prepared: &Prepared,
    request: &WriteRequest<registry::Upgrade>,
    mainnet_plan: &OperationPlan,
    before: &RegistryContents,
    reporter: &mut Reporter,
) -> Result<Option<u64>> {
    let registry_id = &prepared.body.registry_id;
    let (_sandbox, network) = start_sandbox().await?;
    let secret_key = setup_account(
        &network,
        registry_id,
        &prepared.replay_key,
        &prepared.snapshot,
    )
    .await?;
    let client = build_local_client(&network, registry_id, &secret_key)?;
    stage_local_code(&client, registry_id, &prepared.snapshot.code).await?;
    reset_account_metadata(&network, registry_id, &prepared.snapshot).await?;

    let reproduced = read_contents(&client, registry_id).await;
    reporter.record(Check::new(
        "upgrade.replay.fidelity",
        match reproduced {
            Ok(reproduced) if reproduced == *before => {
                Status::passed("the sandbox registry serves exactly what mainnet serves")
            }
            Ok(reproduced) => Status::failed(format!(
                "the sandbox registry differs from mainnet: {}",
                contents_status(before, &reproduced).detail()
            )),
            Err(error) => Status::failed(format!("read the sandbox registry: {error:#}")),
        },
    ));

    let local_plan = client.plan_request(request.clone()).await;
    reporter.record(Check::new(
        "upgrade.replay.transaction",
        match &local_plan {
            Ok(plan) if plan.steps == mainnet_plan.steps => {
                Status::passed("the sandbox plans the same transaction as mainnet")
            }
            Ok(_) => Status::failed("the sandbox plans a different transaction than mainnet"),
            Err(error) => Status::failed(format!("plan in the sandbox: {error}")),
        },
    ));
    if reporter.has_failures() {
        return Ok(None);
    }

    let result = client
        .execute_as(ManagedAccountId(registry_id.clone()), request.body.clone())
        .await;
    reporter.record(Check::new(
        "upgrade.replay.outcome",
        outcome_status(&result),
    ));
    if let Ok(result) = &result {
        reporter.record(Check::new(
            "upgrade.replay.gas",
            gas_status(result, mainnet_plan),
        ));
    }
    let (checks, storage_usage) = verify(
        &client,
        registry_id,
        &prepared.expected,
        before,
        "upgrade.replay",
    )
    .await;
    reporter.extend(checks);
    Ok(storage_usage)
}

fn outcome_status(result: &Result<WriteOperationResult, GatewayError>) -> Status {
    match result {
        Ok(result) if result.operation.status == OperationStatus::Succeeded => {
            Status::passed("the deploy and `migrate` succeeded")
        }
        Ok(result) => Status::failed(format!(
            "{:?}: {}",
            result.operation.status,
            result
                .operation
                .failure_message()
                .unwrap_or("<no failure message>")
        )),
        Err(error) => Status::failed(error.to_string()),
    }
}

/// The replay ran on another protocol version's costs and a shallower trie, so the burn it
/// measures must leave at least half of what `migrate` attaches.
fn gas_status(result: &WriteOperationResult, plan: &OperationPlan) -> Status {
    let attached = attached_gas(plan);
    let attached_tgas = attached.as_tgas();
    let Some(burnt) = result
        .operation
        .final_outcome()
        .map(|outcome| outcome.total_gas_burnt)
    else {
        return Status::failed("the replay reported no outcome");
    };
    if burnt.as_gas().saturating_mul(2) <= attached.as_gas() {
        Status::passed(format!(
            "burnt {} of {attached_tgas} Tgas attached",
            burnt.as_tgas()
        ))
    } else {
        Status::failed(format!(
            "burnt {} Tgas with the deploy, more than half of the {attached_tgas} Tgas attached; \
             prune versions with `registry remove-version` to shrink the migration",
            burnt.as_tgas()
        ))
    }
}

fn attached_gas(plan: &OperationPlan) -> NearGas {
    NearGas::from_gas(
        plan.steps
            .iter()
            .flat_map(|step| &step.actions)
            .filter_map(|action| match action {
                Action::FunctionCall(call) => Some(call.gas.as_gas()),
                _ => None,
            })
            .sum(),
    )
}

/// What must hold of an upgraded registry, asked of the sandbox and then of mainnet.
async fn verify(
    client: &Client,
    registry_id: &AccountId,
    expected: &Expected,
    before: &RegistryContents,
    prefix: &str,
) -> (Vec<Check>, Option<u64>) {
    let (account, state_version, after) = futures::join!(
        client.read(account::Get {
            account_id: registry_id.clone(),
        }),
        client.read(contract::GetStateVersion {
            contract_id: registry_id.clone(),
        }),
        read_contents(client, registry_id),
    );
    let storage_usage = account.as_ref().ok().map(|account| account.storage_usage);
    let (code, storage) = match account {
        Ok(account) => (
            if account.code_hash == expected.code_hash.to_string() {
                Status::passed(format!("runs {}", expected.code_hash))
            } else {
                Status::failed(format!(
                    "runs {}, expected {}",
                    account.code_hash, expected.code_hash
                ))
            },
            storage_status(expected, account.storage_usage),
        ),
        Err(error) => (
            Status::failed(error.to_string()),
            Status::failed(error.to_string()),
        ),
    };
    let state_version = match state_version {
        Ok(version) => state_version_status(version),
        Err(error) => Status::failed(error.to_string()),
    };
    let contents = match after {
        Ok(after) => contents_status(before, &after),
        Err(error) => Status::failed(format!("{error:#}")),
    };
    let checks = [
        ("code", code),
        ("storage", storage),
        ("state_version", state_version),
        ("contents", contents),
    ]
    .into_iter()
    .map(|(leaf, status)| Check::new(format!("{prefix}.{leaf}"), status))
    .collect();
    (checks, storage_usage)
}

/// The views cannot see a stored blob or a reserved name go missing, but storage can: beyond the
/// code swap, a pre-1.1.0 migration adds a byte per version and any adds a state version record.
fn storage_status(expected: &Expected, actual: u64) -> Status {
    let lowest = expected.storage_usage - expected.shrink;
    let highest = expected.storage_usage + expected.growth;
    let delta = i128::from(actual) - i128::from(expected.storage_usage);
    if (lowest..=highest).contains(&actual) {
        Status::passed(format!("{actual} bytes, {delta:+} beyond the code swap"))
    } else {
        Status::failed(format!(
            "{actual} bytes, {delta:+} beyond the code swap; the migration may only move it within \
             {lowest}..={highest}, so stored state was lost or duplicated"
        ))
    }
}

fn state_version_status(version: GetStateVersionResult) -> Status {
    if !version.needs_migration && version.stored == version.target {
        Status::passed(format!("stored state version {}", version.stored))
    } else {
        Status::failed(format!(
            "stored {} against target {}, needs_migration {}",
            version.stored, version.target, version.needs_migration
        ))
    }
}

fn contents_status(before: &RegistryContents, after: &RegistryContents) -> Status {
    let mut differences = Vec::new();
    if before.owner != after.owner {
        differences.push(format!("owner {:?} became {:?}", before.owner, after.owner));
    }
    if before.versions != after.versions {
        differences.push(format!(
            "{} version(s) became {}, or a code hash changed",
            before.versions.len(),
            after.versions.len()
        ));
    }
    if before.deployments != after.deployments {
        differences.push(format!(
            "{} deployment(s) became {}",
            before.deployments.len(),
            after.deployments.len()
        ));
    }
    if differences.is_empty() {
        Status::passed(format!(
            "owner, {} version(s) with their code hashes, and {} deployment(s) unchanged",
            before.versions.len(),
            before.deployments.len()
        ))
    } else {
        Status::failed(differences.join("; "))
    }
}

/// The account must still be what was snapshotted and replayed, byte for byte: a write landing in
/// between would put the transaction on state nothing checked.
async fn drift_status(
    ctx: &CliContext,
    registry_id: &AccountId,
    snapshot: &StateSnapshot,
    limits: &ProtocolLimits,
) -> Status {
    match fetch_state(ctx, &ctx.client, registry_id, limits).await {
        Ok(now) => {
            let at = now.block_hash;
            if unchanged(now, snapshot) {
                Status::passed(format!(
                    "unchanged between {} and {at}",
                    snapshot.block_hash
                ))
            } else {
                Status::failed(format!(
                    "the account changed between {} and {at}; re-run against its current state",
                    snapshot.block_hash
                ))
            }
        }
        Err(error) => Status::failed(format!("{error:#}")),
    }
}

/// Whether `now` is `snapshot` read again: only when and how it was fetched may differ, and the
/// balance may grow, since anyone can send NEAR and the replay does not depend on it.
fn unchanged(mut now: StateSnapshot, snapshot: &StateSnapshot) -> bool {
    now.block_hash = snapshot.block_hash;
    now.request_count = snapshot.request_count;
    if now.amount >= snapshot.amount {
        now.amount = snapshot.amount;
    }
    now == *snapshot
}

/// Read through views every registry release serves, so old and new code answer alike.
async fn read_contents(client: &Client, registry_id: &AccountId) -> Result<RegistryContents> {
    let owner = async {
        client
            .read(owner::GetOwner {
                contract_id: registry_id.clone(),
            })
            .await
            .with_context(|| format!("read the owner of {registry_id}"))
    };
    let versions = async {
        let keys = collect_paginated(PAGE, |offset, limit| {
            client.read(registry::ListVersions {
                registry_id: registry_id.clone(),
                args: page(offset, limit),
            })
        })
        .await
        .with_context(|| format!("list the versions of {registry_id}"))?;
        let hashes: Vec<_> = futures::stream::iter(keys.iter().map(|key| {
            client.read(registry::GetVersionCodeHash {
                registry_id: registry_id.clone(),
                version_key: key.clone(),
            })
        }))
        .buffered(CONCURRENT_READS)
        .try_collect()
        .await
        .with_context(|| format!("read the version code hashes of {registry_id}"))?;
        Ok::<Vec<_>, anyhow::Error>(keys.into_iter().zip(hashes).collect())
    };
    let deployments = async {
        collect_paginated(PAGE, |offset, limit| {
            client.read(registry::ListDeployments {
                registry_id: registry_id.clone(),
                args: page(offset, limit),
            })
        })
        .await
        .with_context(|| format!("list the deployments of {registry_id}"))
    };
    let (owner, mut versions, mut deployments) = futures::try_join!(owner, versions, deployments)?;
    // Compared as sets: a migration may rebuild a collection in another iteration order.
    versions.sort();
    deployments.sort();
    Ok(RegistryContents {
        owner,
        versions,
        deployments,
    })
}

fn page(offset: u32, limit: u32) -> Pagination {
    Pagination {
        offset: Some(offset),
        limit: Some(limit),
    }
}

#[cfg(test)]
mod tests {
    use near_api::types::transaction::actions::{AccessKey, FunctionCallPermission};
    use rstest::rstest;

    use super::*;
    use crate::dispatch::patch_state::RawStateEntry;

    const REGISTRY: &str = "templar-alpha.near";

    fn registry_id() -> AccountId {
        REGISTRY.parse().unwrap()
    }

    fn key(seed: &str) -> near_api::types::PublicKey {
        near_crypto::SecretKey::from_seed(near_crypto::KeyType::ED25519, seed)
            .public_key()
            .to_string()
            .parse()
            .unwrap()
    }

    fn full_access() -> AccessKey {
        AccessKey {
            nonce: 0.into(),
            permission: AccessKeyPermission::FullAccess,
        }
    }

    fn function_call_access() -> AccessKey {
        AccessKey {
            nonce: 0.into(),
            permission: AccessKeyPermission::FunctionCall(FunctionCallPermission {
                allowance: None,
                receiver_id: REGISTRY.parse().unwrap(),
                method_names: Vec::new(),
            }),
        }
    }

    fn snapshot(entry_bytes: &[usize]) -> StateSnapshot {
        StateSnapshot {
            amount: NearToken::from_near(10),
            locked: NearToken::from_yoctonear(0),
            storage_usage: 300_000,
            contract: AccountContract::Local(CryptoHash::default()),
            code: vec![0; 200_000],
            access_keys: vec![
                (key("full"), full_access()),
                (key("function-call"), function_call_access()),
            ],
            entries: entry_bytes
                .iter()
                .enumerate()
                .map(|(index, len)| RawStateEntry {
                    key: vec![u8::try_from(index).unwrap()],
                    value: vec![0; *len],
                })
                .collect(),
            block_hash: near_api::types::CryptoHash::default(),
            request_count: 1,
        }
    }

    #[test]
    fn only_the_registry_can_sign() {
        assert!(!signer_status(&registry_id(), &registry_id()).is_failure());
        assert!(signer_status(&"tmplr.near".parse().unwrap(), &registry_id()).is_failure());
    }

    #[test]
    fn uncatalogued_code_has_no_known_layout() {
        let (status, version) = source_status(&snapshot(&[]), &Ok("1.0.0".to_owned()));
        assert!(version.is_none());
        let Status::Failed { detail } = status else {
            panic!("uncatalogued code must fail");
        };
        assert!(
            detail.contains("not a catalogued registry release"),
            "{detail}"
        );
    }

    #[test]
    fn code_must_be_deployed_locally() {
        let mut state = snapshot(&[]);
        state.contract = AccountContract::None;
        let (status, version) = source_status(&state, &Ok("1.0.0".to_owned()));
        assert!(status.is_failure() && version.is_none(), "{status:?}");
    }

    #[rstest]
    #[case::alpha_near((0, 1, 0), Some(Migration::PreGlobalContracts))]
    #[case::v1_tmplr_near((1, 0, 0), Some(Migration::PreGlobalContracts))]
    #[case::user0_tmplr_near((1, 1, 0), Some(Migration::WithGlobalContracts))]
    #[case((1, 2, 4), Some(Migration::WithGlobalContracts))]
    #[case::already_upgradable((2, 0, 0), None)]
    fn the_migration_follows_from_the_source_release(
        #[case] source: (u64, u64, u64),
        #[case] expected: Option<Migration>,
    ) {
        let (status, migration) = migration_status(RegistryVersion::from(source));
        assert_eq!(migration, expected);
        assert_eq!(status.is_failure(), expected.is_none(), "{status:?}");
    }

    #[rstest]
    #[case::newest(None, false)]
    #[case::named(Some("2.0.0"), false)]
    #[case::uncatalogued(Some("9.9.9"), true)]
    #[case::predates_versioned_state(Some("1.2.4"), true)]
    fn the_target_must_be_a_release_with_versioned_state(
        #[case] requested: Option<&str>,
        #[case] fails: bool,
    ) {
        let (status, release) = target_status(requested);
        assert_eq!(status.is_failure(), fails, "{status:?}");
        assert_eq!(release.is_none(), fails);
    }

    #[test]
    fn the_signing_key_must_hold_full_access() {
        let state = snapshot(&[]);

        let (status, replay) = signing_key_status(&state, &key("full"));
        assert!(!status.is_failure(), "{status:?}");
        assert_eq!(replay, Some(key("full")));

        for key in [key("function-call"), key("stranger")] {
            let (status, replay) = signing_key_status(&state, &key);
            assert!(status.is_failure() && replay.is_none(), "{status:?}");
        }
    }

    #[rstest]
    #[case::ample(NearToken::from_near(10), false)]
    #[case::short(NearToken::from_near(3), true)]
    fn the_balance_must_stake_the_new_code(#[case] amount: NearToken, #[case] fails: bool) {
        let expected = Expected {
            code_hash: CryptoHash::default(),
            storage_usage: 400_000,
            growth: 3,
            shrink: 0,
        };
        // 400 KB stakes 4 NEAR, plus the headroom.
        let status = balance_status(amount, &expected);
        assert_eq!(status.is_failure(), fails, "{status:?}");
    }

    #[rstest]
    #[case::within(Migration::PreGlobalContracts, &[1_000_000, 1_000_000], false)]
    #[case::over(Migration::PreGlobalContracts, &[1_600_000, 1_600_000], true)]
    #[case::rewrites_nothing(Migration::WithGlobalContracts, &[1_600_000, 1_600_000], false)]
    fn the_proof_budget_binds_only_the_migration_that_rewrites_code(
        #[case] migration: Migration,
        #[case] entries: &[usize],
        #[case] fails: bool,
    ) {
        let status = proof_budget_status(migration, &snapshot(entries));
        assert_eq!(status.is_failure(), fails, "{status:?}");
    }

    #[rstest]
    #[case::current(1, 1, false, false)]
    #[case::behind(0, 1, true, true)]
    #[case::stale_flag(1, 1, true, true)]
    fn the_state_version_must_be_current(
        #[case] stored: u32,
        #[case] target: u32,
        #[case] needs_migration: bool,
        #[case] fails: bool,
    ) {
        let status = state_version_status(GetStateVersionResult {
            stored,
            target,
            needs_migration,
        });
        assert_eq!(status.is_failure(), fails, "{status:?}");
    }

    fn hash(byte: u8) -> templar_gateway_types::CryptoHash {
        near_api::types::CryptoHash([byte; 32]).into()
    }

    fn contents() -> RegistryContents {
        RegistryContents {
            owner: Some(registry_id()),
            versions: vec![
                ("market@1.0.0".to_owned(), Some(hash(1))),
                ("market@0.9.0".to_owned(), Some(hash(2))),
            ],
            deployments: vec!["market.templar-alpha.near".parse().unwrap()],
        }
    }

    #[test]
    fn unchanged_contents_pass() {
        assert!(!contents_status(&contents(), &contents()).is_failure());
    }

    #[rstest]
    #[case::owner(|c: &mut RegistryContents| c.owner = None, "owner")]
    #[case::code_hash(|c: &mut RegistryContents| c.versions[0].1 = None, "version")]
    #[case::lost_version(|c: &mut RegistryContents| { c.versions.pop(); }, "version")]
    #[case::lost_deployment(|c: &mut RegistryContents| c.deployments.clear(), "deployment")]
    fn any_change_to_the_contents_fails(
        #[case] change: fn(&mut RegistryContents),
        #[case] named: &str,
    ) {
        let mut after = contents();
        change(&mut after);
        let Status::Failed { detail } = contents_status(&contents(), &after) else {
            panic!("a changed registry must fail");
        };
        assert!(detail.contains(named), "{detail}");
    }

    fn expected() -> Expected {
        Expected {
            code_hash: CryptoHash::default(),
            storage_usage: 1_000_000,
            growth: 356,
            shrink: 64,
        }
    }

    /// A lost blob or reserved name shows only as storage, so the band has to be tight.
    #[rstest]
    #[case::unchanged(1_000_000, false)]
    #[case::discriminants_and_version_record(1_000_000 + 356, false)]
    #[case::retired_collection(1_000_000 - 64, false)]
    #[case::lost_reserved_name(1_000_000 - 150, true)]
    #[case::lost_blob(1_000_000 - 300_000, true)]
    #[case::grew_beyond_the_migration(1_000_000 + 357, true)]
    fn storage_may_only_move_by_what_the_migration_writes(
        #[case] actual: u64,
        #[case] fails: bool,
    ) {
        let status = storage_status(&expected(), actual);
        assert_eq!(status.is_failure(), fails, "{status:?}");
    }

    /// A preflight that stops on an error it could not record must still fail, never exit 0.
    #[test]
    fn stopping_before_submission_fails_even_with_no_failed_check() {
        let mut reporter = Reporter::capturing(&[]).quieted();
        reporter.record(Check::new("upgrade.network", Status::passed("mainnet")));

        let error = finish(&registry_id(), reporter).expect_err("nothing was submitted");
        assert!(error.to_string().contains("not submitted"), "{error}");
    }

    /// A legacy `registry` map holding `tags`, one name per entry, as near-sdk lays it out.
    fn with_names(tags: &[Option<u8>]) -> StateSnapshot {
        let mut state = snapshot(&[10]);
        for (index, tag) in (0u32..).zip(tags) {
            let name = borsh::to_vec(&format!("m{index}.registry.near")).unwrap();
            state.entries.push(RawStateEntry {
                key: [REGISTRY_KEYS_PREFIX, &index.to_le_bytes()].concat(),
                value: name.clone(),
            });
            if let Some(tag) = tag {
                state.entries.push(RawStateEntry {
                    key: CryptoHash::hash_bytes(&[REGISTRY_VALUES_PREFIX, &name].concat())
                        .0
                        .to_vec(),
                    value: vec![*tag, 1, 2, 3],
                });
            }
        }
        state
    }

    #[rstest]
    #[case::all_deployed(&[Some(1), Some(1)], 2, false)]
    #[case::one_reserved(&[Some(1), Some(RESERVED_TAG)], 1, true)]
    #[case::unknown_layout(&[Some(1), None], 2, true)]
    #[case::nothing_decoded(&[], 2, true)]
    fn a_reserved_name_blocks_the_upgrade(
        #[case] tags: &[Option<u8>],
        #[case] listed: usize,
        #[case] fails: bool,
    ) {
        let status = reserved_status(&with_names(tags), listed);
        assert_eq!(status.is_failure(), fails, "{status:?}");
    }

    #[test]
    fn a_snapshot_read_again_is_unchanged() {
        let mut now = snapshot(&[10, 20]);
        now.block_hash = near_api::types::CryptoHash([9; 32]);
        now.request_count = 7;
        assert!(unchanged(now, &snapshot(&[10, 20])));
    }

    #[test]
    fn a_balance_may_grow_but_not_shrink() {
        let mut richer = snapshot(&[10]);
        richer.amount = richer.amount.saturating_add(NearToken::from_near(1));
        assert!(unchanged(richer, &snapshot(&[10])));

        let mut poorer = snapshot(&[10]);
        poorer.amount = poorer.amount.saturating_sub(NearToken::from_yoctonear(1));
        assert!(!unchanged(poorer, &snapshot(&[10])));
    }

    #[rstest]
    #[case::storage(|s: &mut StateSnapshot| s.entries[0].value.push(0))]
    #[case::code(|s: &mut StateSnapshot| s.code.push(0))]
    #[case::key(|s: &mut StateSnapshot| { s.access_keys.pop(); })]
    #[case::nonce(|s: &mut StateSnapshot| s.access_keys[0].1.nonce = 9.into())]
    #[case::locked(|s: &mut StateSnapshot| s.locked = NearToken::from_near(1))]
    fn any_other_change_is_drift(#[case] change: fn(&mut StateSnapshot)) {
        let mut now = snapshot(&[10]);
        change(&mut now);
        assert!(!unchanged(now, &snapshot(&[10])));
    }

    #[rstest]
    #[case::pre_global_contracts(Migration::PreGlobalContracts, 3 + VERSION_RECORD_ALLOWANCE, 0)]
    #[case::with_global_contracts(
        Migration::WithGlobalContracts,
        VERSION_RECORD_ALLOWANCE,
        RETIRED_COLLECTION_ALLOWANCE
    )]
    fn the_storage_band_follows_the_migration(
        #[case] migration: Migration,
        #[case] growth: u64,
        #[case] shrink: u64,
    ) {
        let mut state = snapshot(&[10]);
        // Three version keys, and a hashed value key that happens to start with their prefix.
        for index in 0u32..3 {
            state.entries.push(RawStateEntry {
                key: [VERSION_KEYS_PREFIX, &index.to_le_bytes()].concat(),
                value: vec![0; 8],
            });
        }
        state.entries.push(RawStateEntry {
            key: [VERSION_KEYS_PREFIX, &[0; 30]].concat(),
            value: vec![0; 8],
        });
        let wasm = vec![0; 250_000];

        let expected = expected_after(&state, migration, &wasm);

        assert_eq!(expected.storage_usage, 300_000 - 200_000 + 250_000);
        assert_eq!(expected.growth, growth);
        assert_eq!(expected.shrink, shrink);
        assert_eq!(expected.code_hash, CryptoHash::hash_bytes(&wasm));
    }

    #[test]
    fn mainnet_must_land_exactly_on_the_replayed_storage() {
        let exact = expected().exactly(1_000_123);
        assert!(!storage_status(&exact, 1_000_123).is_failure());
        assert!(storage_status(&exact, 1_000_122).is_failure());
        assert!(storage_status(&exact, 1_000_124).is_failure());
    }

    fn plan_attaching(tgas: u64) -> OperationPlan {
        OperationPlan::single(templar_gateway_core::PlannedTransaction::single_action(
            ManagedAccountId(registry_id()),
            registry_id(),
            Action::FunctionCall(Box::new(
                near_api::types::transaction::actions::FunctionCallAction {
                    method_name: "migrate".to_owned(),
                    args: Vec::new(),
                    gas: NearGas::from_tgas(tgas),
                    deposit: NearToken::from_yoctonear(0),
                },
            )),
        ))
    }

    fn burnt(tgas: u64) -> WriteOperationResult {
        use templar_gateway_types::operation::{
            ExecutionOutcome, OperationId, OperationRecord, StepStatus, TransactionStepRecord,
        };
        OperationRecord {
            id: OperationId("op-1".to_owned()),
            signer_account_id: ManagedAccountId(registry_id()),
            status: OperationStatus::Succeeded,
            steps: vec![TransactionStepRecord {
                index: 0,
                status: StepStatus::Succeeded {
                    tx_hash: near_api::types::CryptoHash::default().into(),
                    outcome: ExecutionOutcome {
                        tokens_burnt: NearToken::from_yoctonear(0),
                        total_gas_burnt: NearGas::from_tgas(tgas),
                        receipts: Vec::new(),
                        return_value: None,
                        failure: None,
                    },
                },
            }],
        }
        .into()
    }

    #[rstest]
    #[case::well_within(176, false)]
    #[case::exactly_half(300, false)]
    #[case::over_half(301, true)]
    fn the_replay_must_leave_half_the_attached_gas(#[case] tgas: u64, #[case] fails: bool) {
        let status = gas_status(&burnt(tgas), &plan_attaching(600));
        assert_eq!(status.is_failure(), fails, "{status:?}");
    }
}
