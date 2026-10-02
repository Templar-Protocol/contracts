use async_trait::async_trait;
use near_account_id::AccountId;
use near_api::types::transaction::actions::Action;
use serde::Serialize;
use templar_common::upgrade::MIGRATE_METHOD;
use templar_gateway_core::{
    client::registry::{
        AddVersionArgs, DeployArgs, GetDeploymentArgs, GetRegistryEntryArgs, GetVersionArgs,
        RemoveVersionArgs,
    },
    query_contract_kind, ContractWriteOptions, DispatchRead, GatewayError, GatewayResult,
    HasNearClient, OperationPlan, PlanWrite,
};
use templar_gateway_methods_spec::registry;
use templar_gateway_types::{NearGas, NearToken, Registry, RegistryVersion};

use crate::{
    contract_abi::validate_constructor_args, registry_wasm::resolve_registry_wasm,
    tx_impl::deploy_and_call_actions, Dispatch,
};

#[async_trait]
impl<C> DispatchRead<registry::ListDeployments, C> for Dispatch
where
    C: HasNearClient,
{
    async fn dispatch(request: registry::ListDeployments, ctx: C) -> GatewayResult<Vec<AccountId>> {
        ctx.near_client()
            .registry(request.registry_id)
            .list_deployments(request.args)
            .await
    }
}

#[async_trait]
impl<C: HasNearClient> DispatchRead<registry::GetDeployment, C> for Dispatch {
    async fn dispatch(
        request: registry::GetDeployment,
        ctx: C,
    ) -> GatewayResult<Option<templar_common::registry::Deployment>> {
        ctx.near_client()
            .registry(request.registry_id)
            .get_deployment(GetDeploymentArgs {
                account_id: request.account_id,
            })
            .await
    }
}

/// Refuse a registry too old to serve `get_registry_entry` / `get_version`, rather than answering
/// from the views it does have.
///
/// Both exist to report a state their predecessors collapse — a reserved name, a version whose
/// code was removed. Synthesising either from `get_deployment` or `list_versions` would return the
/// very answer they were added to correct, and return it indistinguishably from a real one. A
/// caller that can degrade should ask the version first, as `tmplrmgr`'s preflight does.
async fn require_entry_and_version_views<C: HasNearClient>(
    ctx: &C,
    registry_id: near_account_id::AccountId,
) -> GatewayResult<()> {
    let version: RegistryVersion = ctx
        .near_client()
        .contract(registry_id.clone())
        .cached_version()
        .await?;

    if !version.supports_entry_and_version_views() {
        return Err(GatewayError::UnsupportedFeature(format!(
            "registry {registry_id} is version {version}; \
             getRegistryEntry and getVersion require 2.0.0"
        )));
    }

    Ok(())
}

#[async_trait]
impl<C: HasNearClient> DispatchRead<registry::GetRegistryEntry, C> for Dispatch {
    async fn dispatch(
        request: registry::GetRegistryEntry,
        ctx: C,
    ) -> GatewayResult<Option<templar_common::registry::RegistryEntryView>> {
        require_entry_and_version_views(&ctx, request.registry_id.clone()).await?;
        ctx.near_client()
            .registry(request.registry_id)
            .get_registry_entry(GetRegistryEntryArgs {
                account_id: request.account_id,
            })
            .await
    }
}

#[async_trait]
impl<C: HasNearClient> DispatchRead<registry::GetVersion, C> for Dispatch {
    async fn dispatch(
        request: registry::GetVersion,
        ctx: C,
    ) -> GatewayResult<Option<templar_common::registry::VersionInfo>> {
        require_entry_and_version_views(&ctx, request.registry_id.clone()).await?;
        ctx.near_client()
            .registry(request.registry_id)
            .get_version(GetVersionArgs {
                version_key: request.version_key,
            })
            .await
    }
}

#[async_trait]
impl<C: HasNearClient> DispatchRead<registry::GetVersionCodeHash, C> for Dispatch {
    async fn dispatch(
        request: registry::GetVersionCodeHash,
        ctx: C,
    ) -> GatewayResult<Option<templar_gateway_types::CryptoHash>> {
        let hash = ctx
            .near_client()
            .registry(request.registry_id)
            .get_version_code_hash(GetVersionArgs {
                version_key: request.version_key,
            })
            .await?;
        Ok(hash.map(|hash| near_api::types::CryptoHash(hash.into()).into()))
    }
}

#[async_trait]
impl<C: HasNearClient> DispatchRead<registry::ListVersions, C> for Dispatch {
    async fn dispatch(request: registry::ListVersions, ctx: C) -> GatewayResult<Vec<String>> {
        ctx.near_client()
            .registry(request.registry_id)
            .list_versions(request.args)
            .await
    }
}

#[async_trait]
impl<C> DispatchRead<registry::ListDeploymentsByKind, C> for Dispatch
where
    C: HasNearClient,
{
    async fn dispatch(
        request: registry::ListDeploymentsByKind,
        ctx: C,
    ) -> GatewayResult<Vec<AccountId>> {
        let account_ids = ctx
            .near_client()
            .registry(request.registry_id)
            .list_deployments(templar_gateway_types::common::Pagination::default())
            .await?;

        let mut filtered = Vec::new();
        for account_id in account_ids {
            if query_contract_kind(&ctx, account_id.clone()).await? == request.kind {
                filtered.push(account_id);
            }
        }

        let offset = request.args.offset.unwrap_or_default() as usize;
        let limit = request.args.limit.map(|value| value as usize);
        let account_ids = if let Some(limit) = limit {
            filtered.into_iter().skip(offset).take(limit).collect()
        } else {
            filtered.into_iter().skip(offset).collect()
        };

        Ok(account_ids)
    }
}

#[async_trait]
impl<C: HasNearClient> PlanWrite<registry::AddVersion, C> for Dispatch {
    async fn plan(
        request: templar_gateway_types::common::WriteRequest<registry::AddVersion>,
        ctx: C,
    ) -> GatewayResult<OperationPlan> {
        let body = request.body;
        let registry_version = ctx
            .near_client()
            .contract(body.registry_id.clone())
            .cached_version()
            .await?;
        ctx.near_client()
            .registry(body.registry_id)
            .add_version(
                ContractWriteOptions::new(request.signer_account_id)
                    .tgas(300)
                    .deposit(body.deposit),
                registry_version,
                AddVersionArgs {
                    version_key: body.version_key,
                    source: body.source,
                },
            )
            .map(OperationPlan::from)
    }
}

#[async_trait]
impl<C> PlanWrite<registry::Deploy, C> for Dispatch
where
    C: HasNearClient,
{
    async fn plan(
        request: templar_gateway_types::common::WriteRequest<registry::Deploy>,
        ctx: C,
    ) -> GatewayResult<OperationPlan> {
        let registry::Deploy { target, init_args } = request.body;
        plan_create_from_registry(&ctx, request.signer_account_id, target, init_args.0).await
    }
}

/// Plan the deploy of a registered version to a new sub-account under the registry.
///
/// Encode `init_args` with `serde_json` unless the payload holds a `HashMap`, whose
/// order varies per process: JSON Canonicalization sorts keys but rounds every
/// integer through `f64`, silently altering any `u64` past 2^53.
pub(crate) async fn plan_create_from_registry<C: HasNearClient>(
    ctx: &C,
    signer_account_id: templar_gateway_types::ManagedAccountId,
    target: registry::DeployTarget,
    mut init_args: Vec<u8>,
) -> GatewayResult<OperationPlan> {
    let registry_version = ctx
        .near_client()
        .contract(target.registry_id.clone())
        .cached_version()
        .await?;
    if !target.skip_abi_check {
        let wasm = resolve_registry_wasm(
            ctx,
            &target.registry_id,
            registry_version,
            &target.version_key,
        )
        .await?;
        init_args = validate_constructor_args(wasm, init_args).await?;
    }

    Ok(OperationPlan::single(
        ctx.near_client().registry(target.registry_id).deploy(
            ContractWriteOptions::new(signer_account_id)
                .tgas(300)
                .deposit(target.deposit),
            registry_version,
            DeployArgs {
                name: target.name,
                version_key: target.version_key,
                init_args: init_args.into(),
                full_access_keys: target
                    .full_access_keys
                    .map(|keys| keys.into_iter().map(Into::into).collect()),
            },
        )?,
    ))
}

#[async_trait]
impl<C: HasNearClient> PlanWrite<registry::RemoveVersion, C> for Dispatch {
    async fn plan(
        request: templar_gateway_types::common::WriteRequest<registry::RemoveVersion>,
        ctx: C,
    ) -> GatewayResult<OperationPlan> {
        let body = request.body;
        ctx.near_client()
            .registry(body.registry_id)
            .remove_version(
                ContractWriteOptions::new(request.signer_account_id)
                    .tgas(300)
                    .one_yocto(),
                RemoveVersionArgs {
                    version_key: body.version_key,
                },
            )
            .map(OperationPlan::from)
    }
}

/// What the deployed `migrate` reads; the tag is the contract's `state::Migration` wire format.
#[derive(Serialize)]
struct MigrateArgs {
    from_version: registry::Migration,
}

/// The most a sandbox replay can prove: a sandbox caps a transaction at 300 Tgas.
const MIGRATE_GAS: NearGas = NearGas::from_tgas(280);

fn upgrade_actions(
    body: registry::Upgrade,
    version: RegistryVersion,
) -> GatewayResult<Vec<Action>> {
    let registry::Upgrade {
        registry_id,
        wasm,
        migration,
    } = body;
    match registry::Migration::for_version(version) {
        Some(required) if required == migration => {}
        Some(required) => {
            return Err(GatewayError::RequestPreconditionFailed(format!(
                "registry {registry_id} is version {version}, which needs migration {required:?}; \
                 got {migration:?}"
            )))
        }
        None => {
            return Err(GatewayError::UnsupportedFeature(format!(
                "registry {registry_id} is version {version} and replaces its own code through \
                 its owner-only `upgrade` method"
            )))
        }
    }

    Ok(deploy_and_call_actions(
        wasm.0,
        MIGRATE_METHOD.to_owned(),
        serde_json::to_vec(&MigrateArgs {
            from_version: migration,
        })?,
        MIGRATE_GAS,
        NearToken::from_yoctonear(0),
    ))
}

/// Nothing can roll this upgrade back, so only a released registry that has `migrate` may land.
fn require_upgradable_release(wasm: &[u8]) -> GatewayResult<()> {
    let hash = near_api::types::CryptoHash::hash(wasm);
    match templar_contract_artifacts::release_by_sha256(&hash.0) {
        Some((templar_contract_artifacts::ArtifactId::Registry, release))
            if release
                .version
                .parse::<RegistryVersion>()
                .is_ok_and(RegistryVersion::supports_upgrade) =>
        {
            Ok(())
        }
        _ => Err(GatewayError::RequestPreconditionFailed(format!(
            "the wasm ({hash}) is not a catalogued registry release with versioned state"
        ))),
    }
}

#[async_trait]
impl<C: HasNearClient> PlanWrite<registry::Upgrade, C> for Dispatch {
    async fn plan(
        request: templar_gateway_types::common::WriteRequest<registry::Upgrade>,
        ctx: C,
    ) -> GatewayResult<OperationPlan> {
        let registry_id = request.body.registry_id.clone();
        // `DeployContract` needs the account's own key, and `migrate` admits only the account itself.
        if request.signer_account_id.0 != registry_id {
            return Err(GatewayError::RequestPreconditionFailed(format!(
                "a registry upgrade must be signed by the registry account {registry_id}; got {}",
                request.signer_account_id.0
            )));
        }
        require_upgradable_release(&request.body.wasm.0)?;
        // Uncached: the code this plan replaces is exactly what a stale cached version would hide.
        let version = ctx
            .near_client()
            .contract(registry_id.clone())
            .version::<Registry>()
            .await?;
        let actions = upgrade_actions(request.body, version)?;

        Ok(OperationPlan::execute(
            request.signer_account_id,
            registry_id,
            actions,
        ))
    }
}

#[cfg(test)]
mod tests {
    use rstest::rstest;
    use templar_gateway_types::{common::WriteRequest, Base64Bytes, ManagedAccountId};

    use super::*;
    use crate::test_ctx::{offline_ctx, TestCtx};

    const REGISTRY: &str = "registry.near";

    fn body(migration: registry::Migration) -> registry::Upgrade {
        registry::Upgrade {
            registry_id: REGISTRY.parse().unwrap(),
            wasm: Base64Bytes(b"\0asm\x01\0\0\0registry".to_vec()),
            migration,
        }
    }

    #[rstest]
    #[case::alpha_near((0, 1, 0), registry::Migration::PreGlobalContracts)]
    #[case::v1_tmplr_near((1, 0, 0), registry::Migration::PreGlobalContracts)]
    #[case::user0_tmplr_near((1, 1, 0), registry::Migration::WithGlobalContracts)]
    #[case((1, 2, 4), registry::Migration::WithGlobalContracts)]
    fn deploys_the_wasm_then_migrates_from_the_layout_its_version_holds(
        #[case] version: (u64, u64, u64),
        #[case] migration: registry::Migration,
    ) {
        let body = body(migration);
        let wasm = body.wasm.0.clone();
        let actions = upgrade_actions(body, RegistryVersion::from(version))
            .expect("the migration matches the version");

        let [Action::DeployContract(deploy), Action::FunctionCall(call)] = actions.as_slice()
        else {
            panic!("expected a deploy then a function call, got {actions:?}");
        };
        assert_eq!(deploy.code, wasm);
        assert_eq!(call.method_name, "migrate");
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&call.args).unwrap(),
            serde_json::to_value(MigrateArgs {
                from_version: migration
            })
            .unwrap(),
        );
        assert_eq!(call.gas, MIGRATE_GAS);
        assert_eq!(call.deposit.as_yoctonear(), 0);
    }

    /// The registry contract's own `migration_wire_format` test pins the same strings.
    #[rstest]
    #[case(
        registry::Migration::PreGlobalContracts,
        br#"{"from_version":"pre_global_contracts"}"#
    )]
    #[case(
        registry::Migration::WithGlobalContracts,
        br#"{"from_version":"with_global_contracts"}"#
    )]
    fn migrate_args_match_the_contract_wire_format(
        #[case] migration: registry::Migration,
        #[case] expected: &[u8],
    ) {
        assert_eq!(
            serde_json::to_vec(&MigrateArgs {
                from_version: migration
            })
            .unwrap(),
            expected,
        );
    }

    #[rstest]
    #[case::pre_global_on_1_1_0((1, 1, 0), registry::Migration::PreGlobalContracts)]
    #[case::with_global_on_1_0_0((1, 0, 0), registry::Migration::WithGlobalContracts)]
    fn refuses_a_migration_the_version_does_not_need(
        #[case] version: (u64, u64, u64),
        #[case] migration: registry::Migration,
    ) {
        let error = upgrade_actions(body(migration), RegistryVersion::from(version))
            .expect_err("a mismatched migration must not be planned");
        assert!(
            error.to_string().contains("which needs migration"),
            "{error}"
        );
    }

    #[test]
    fn refuses_a_registry_that_upgrades_itself() {
        let error = upgrade_actions(
            body(registry::Migration::WithGlobalContracts),
            RegistryVersion::from((2, 0, 0)),
        )
        .expect_err("2.0.0 has `upgrade` and no legacy layout");
        assert!(
            error.to_string().contains("owner-only `upgrade`"),
            "{error}"
        );
    }

    #[tokio::test]
    async fn refuses_another_signer_before_reading_the_version() {
        let request = WriteRequest {
            signer_account_id: ManagedAccountId("owner.near".parse().unwrap()),
            idempotency_key: None,
            body: body(registry::Migration::PreGlobalContracts),
        };
        let error =
            <Dispatch as PlanWrite<registry::Upgrade, TestCtx>>::plan(request, offline_ctx())
                .await
                .expect_err("only the registry account can deploy to itself");
        assert!(
            error
                .to_string()
                .contains("must be signed by the registry account"),
            "{error}"
        );
    }

    #[rstest]
    #[case::versioned_state("2.0.0", true)]
    #[case::predates_it("1.2.4", false)]
    #[tokio::test]
    async fn only_a_released_registry_with_versioned_state_may_land(
        #[case] release: &str,
        #[case] accepted: bool,
    ) {
        let wasm = templar_contract_artifacts::fetch::released_bytes(
            templar_contract_artifacts::ArtifactId::Registry,
            release,
        )
        .await
        .unwrap();
        assert_eq!(require_upgradable_release(&wasm).is_ok(), accepted);
    }

    #[test]
    fn uncatalogued_code_may_not_land() {
        let error = require_upgradable_release(b"\0asm\x01\0\0\0registry")
            .expect_err("unreleased bytes must not replace a registry");
        assert!(error.to_string().contains("not a catalogued"), "{error}");
    }
}
