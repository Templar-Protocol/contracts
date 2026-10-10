use async_trait::async_trait;
use near_account_id::AccountId;
use near_api::types::account::ContractState;
use templar_common::upgrade::UpgradeSource;
use templar_gateway_core::{
    client::registry::{
        AddVersionArgs, DeployArgs, GetDeploymentArgs, GetRegistryEntryArgs, GetVersionArgs,
        RemoveVersionArgs, UpgradeArgs,
    },
    query_contract_kind, ContractWriteOptions, DispatchRead, GatewayError, GatewayResult,
    HasNearClient, OperationPlan, PlanWrite,
};
use templar_gateway_methods_spec::registry;
use templar_gateway_types::{Base64Bytes, NearGas, Registry, RegistryVersion};

use crate::{
    contract_abi::validate_constructor_args, registry_wasm::resolve_registry_wasm, Dispatch,
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

#[async_trait]
impl<C: HasNearClient> DispatchRead<registry::GetRegistryEntry, C> for Dispatch {
    async fn dispatch(
        request: registry::GetRegistryEntry,
        ctx: C,
    ) -> GatewayResult<Option<templar_common::registry::RegistryEntryView>> {
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
        ctx.near_client()
            .registry(request.registry_id)
            .get_version(GetVersionArgs {
                version_key: request.version_key,
            })
            .await
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
        ctx.near_client()
            .registry(body.registry_id)
            .add_version(
                ContractWriteOptions::new(request.signer_account_id)
                    .tgas(300)
                    .deposit(body.deposit),
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
    if !target.skip_abi_check {
        let wasm = resolve_registry_wasm(ctx, &target.registry_id, &target.version_key).await?;
        init_args = validate_constructor_args(wasm, init_args).await?;
    }

    Ok(OperationPlan::single(
        ctx.near_client().registry(target.registry_id).deploy(
            ContractWriteOptions::new(signer_account_id)
                .tgas(300)
                .deposit(target.deposit),
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

/// `upgrade` reserves 250 Tgas for `migrate`; the rest decodes the code and deploys it.
const UPGRADE_GAS: NearGas = NearGas::from_tgas(500);

/// The catalogued registry release whose bytes hash to `sha256`.
pub fn registry_release(sha256: &[u8; 32]) -> Option<RegistryVersion> {
    match templar_contract_artifacts::release_by_sha256(sha256) {
        Some((templar_contract_artifacts::ArtifactId::Registry, release)) => {
            release.version.parse().ok()
        }
        _ => None,
    }
}

/// A global contract is keyed by its code's sha256, so either kind of deployed code names a release;
/// one linked by account names no fixed code.
fn code_sha256(state: &ContractState) -> Option<[u8; 32]> {
    match state {
        ContractState::LocalHash(hash) | ContractState::GlobalHash(hash) => Some(hash.0),
        ContractState::GlobalAccountId(_) | ContractState::None => None,
    }
}

/// The release the registry runs: its code must be catalogued and its metadata must name it.
fn deployed_release(
    registry_id: &AccountId,
    deployed_sha256: Option<[u8; 32]>,
    reported: RegistryVersion,
) -> GatewayResult<RegistryVersion> {
    match deployed_sha256.as_ref().and_then(registry_release) {
        Some(release) if release == reported => Ok(release),
        Some(release) => Err(GatewayError::RequestPreconditionFailed(format!(
            "registry {registry_id} runs the code of registry {release} but reports {reported}"
        ))),
        None => Err(GatewayError::RequestPreconditionFailed(format!(
            "registry {registry_id} does not run a catalogued registry release"
        ))),
    }
}

/// Nothing rolls an upgrade back, so only catalogued code may land.
fn target_release(code: &UpgradeSource) -> GatewayResult<RegistryVersion> {
    let sha256 = match code {
        UpgradeSource::Code(blob) => near_api::types::CryptoHash::hash(&blob.0).0,
        UpgradeSource::GlobalHash(hash) => (*hash).into(),
    };
    registry_release(&sha256).ok_or_else(|| {
        GatewayError::RequestPreconditionFailed(format!(
            "the code ({}) is not a catalogued registry release",
            near_api::types::CryptoHash(sha256)
        ))
    })
}

fn require_no_downgrade(target: RegistryVersion, deployed: RegistryVersion) -> GatewayResult<()> {
    if target >= deployed {
        Ok(())
    } else {
        Err(GatewayError::RequestPreconditionFailed(format!(
            "registry {target} is older than the deployed {deployed}"
        )))
    }
}

#[async_trait]
impl<C: HasNearClient> PlanWrite<registry::Upgrade, C> for Dispatch {
    async fn plan(
        request: templar_gateway_types::common::WriteRequest<registry::Upgrade>,
        ctx: C,
    ) -> GatewayResult<OperationPlan> {
        let registry::Upgrade {
            registry_id,
            code,
            migrate_args,
        } = request.body;
        let target = target_release(&code)?;
        let client = ctx.near_client();
        // Uncached: the code this plan replaces is exactly what a stale cached version would hide.
        let version = client
            .contract(registry_id.clone())
            .version::<Registry>()
            .await?;
        let account = client.account().get(registry_id.clone()).await?;
        let owner = client.owner(registry_id.clone()).own_get_owner(()).await?;
        if owner.as_ref() != Some(&request.signer_account_id.0) {
            return Err(GatewayError::RequestPreconditionFailed(format!(
                "registry {registry_id} is owned by {owner:?}, so {} cannot upgrade it",
                request.signer_account_id.0
            )));
        }
        require_no_downgrade(
            target,
            deployed_release(&registry_id, code_sha256(&account.contract_state), version)?,
        )?;

        client
            .registry(registry_id)
            .upgrade(
                ContractWriteOptions::new(request.signer_account_id)
                    .gas(UPGRADE_GAS)
                    .one_yocto(),
                UpgradeArgs {
                    code,
                    migrate_args: migrate_args.unwrap_or(Base64Bytes(Vec::new())),
                },
            )
            .map(OperationPlan::from)
    }
}

#[cfg(test)]
mod tests {
    use near_sdk::json_types::{Base58CryptoHash, Base64VecU8};
    use rstest::rstest;
    use templar_gateway_types::{common::WriteRequest, ManagedAccountId};

    use super::*;
    use crate::test_ctx::{offline_ctx, TestCtx};

    const REGISTRY: &str = "registry.near";

    fn catalogued(release: &str) -> [u8; 32] {
        let sha256 = templar_contract_artifacts::ArtifactId::Registry
            .metadata()
            .release(release)
            .expect("a catalogued registry release")
            .sha256;
        let mut bytes = [0u8; 32];
        for (byte, pair) in bytes.iter_mut().zip(sha256.as_bytes().chunks(2)) {
            *byte = u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap();
        }
        bytes
    }

    #[test]
    fn a_catalogued_global_hash_names_its_release() {
        let code = UpgradeSource::GlobalHash(Base58CryptoHash::from(catalogued("2.0.0")));
        assert_eq!(
            target_release(&code).unwrap(),
            RegistryVersion::from((2, 0, 0))
        );
    }

    #[rstest]
    #[case::blob(UpgradeSource::Code(Base64VecU8(b"\0asm\x01\0\0\0registry".to_vec())))]
    #[case::global_hash(UpgradeSource::GlobalHash(Base58CryptoHash::from([7; 32])))]
    fn uncatalogued_code_may_not_land(#[case] code: UpgradeSource) {
        let error =
            target_release(&code).expect_err("unreleased bytes must not replace a registry");
        assert!(error.to_string().contains("not a catalogued"), "{error}");
    }

    /// Refused before any RPC, which the offline context would fail.
    #[tokio::test]
    async fn uncatalogued_code_is_refused_before_reading_the_registry() {
        let request = WriteRequest {
            signer_account_id: ManagedAccountId(REGISTRY.parse().unwrap()),
            idempotency_key: None,
            body: registry::Upgrade {
                registry_id: REGISTRY.parse().unwrap(),
                code: UpgradeSource::GlobalHash(Base58CryptoHash::from([7; 32])),
                migrate_args: None,
            },
        };
        let error =
            <Dispatch as PlanWrite<registry::Upgrade, TestCtx>>::plan(request, offline_ctx())
                .await
                .expect_err("unreleased bytes must not replace a registry");
        assert!(error.to_string().contains("not a catalogued"), "{error}");
    }

    #[rstest]
    #[case::newer((2, 1, 0), true)]
    #[case::same((2, 0, 0), true)]
    #[case::older((1, 2, 4), false)]
    fn an_upgrade_may_not_downgrade(#[case] target: (u64, u64, u64), #[case] accepted: bool) {
        let result = require_no_downgrade(
            RegistryVersion::from(target),
            RegistryVersion::from((2, 0, 0)),
        );
        assert_eq!(result.is_ok(), accepted, "{result:?}");
    }

    #[rstest]
    #[case::agrees("2.0.0", (2, 0, 0), true)]
    #[case::metadata_disagrees("2.0.0", (2, 0, 1), false)]
    fn the_deployed_code_must_be_the_release_its_metadata_names(
        #[case] code: &str,
        #[case] reported: (u64, u64, u64),
        #[case] accepted: bool,
    ) {
        let result = deployed_release(
            &REGISTRY.parse().unwrap(),
            Some(catalogued(code)),
            RegistryVersion::from(reported),
        );
        assert_eq!(result.is_ok(), accepted, "{result:?}");
    }

    #[rstest]
    #[case::local(ContractState::LocalHash(near_api::types::CryptoHash([1; 32])), Some([1; 32]))]
    #[case::global_hash(ContractState::GlobalHash(near_api::types::CryptoHash([2; 32])), Some([2; 32]))]
    #[case::global_account(ContractState::GlobalAccountId(REGISTRY.parse().unwrap()), None)]
    #[case::no_code(ContractState::None, None)]
    fn local_and_global_code_both_name_a_release(
        #[case] state: ContractState,
        #[case] expected: Option<[u8; 32]>,
    ) {
        assert_eq!(code_sha256(&state), expected);
    }

    #[rstest]
    #[case::uncatalogued(Some([7; 32]))]
    #[case::not_local_code(None)]
    fn unknown_deployed_code_is_refused(#[case] deployed: Option<[u8; 32]>) {
        let error = deployed_release(
            &REGISTRY.parse().unwrap(),
            deployed,
            RegistryVersion::from((2, 0, 0)),
        )
        .expect_err("unknown code must not be upgraded");
        assert!(
            error.to_string().contains("not run a catalogued"),
            "{error}"
        );
    }
}
