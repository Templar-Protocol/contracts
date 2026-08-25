use near_account_id::AccountId;
use near_api::types::CryptoHash;
use sha2::{Digest, Sha256};
use templar_common::registry::VersionAvailability;
use templar_contract_artifacts::fetch;
use templar_gateway_core::{
    client::registry::{GetVersionArgs, GetVersionCodeChunkArgs},
    GatewayError, GatewayResult, HasNearClient, ReadNear,
};
use templar_gateway_types::RegistryVersion;

const CODE_CHUNK_LEN: u32 = 64 * 1024;
const MAX_STORED_CODE_LEN: u32 = 4 * 1024 * 1024;

#[derive(Debug, PartialEq, Eq)]
enum OnchainCodeSource {
    Global,
    Stored(StoredCodeLen),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct StoredCodeLen(u32);

pub(crate) async fn resolve_registry_wasm<C: HasNearClient>(
    ctx: &C,
    registry_id: &AccountId,
    registry_version: RegistryVersion,
    version_key: &str,
) -> GatewayResult<Vec<u8>> {
    let client = ctx.near_client().registry(registry_id.clone());

    if !registry_version.supports_entry_and_version_views() {
        return resolve_legacy_registry_wasm(ctx, &client, version_key).await;
    }

    let Some(version) = client
        .get_version(GetVersionArgs {
            version_key: version_key.to_owned(),
        })
        .await?
    else {
        return Err(precondition(format!(
            "registry version {version_key} does not exist"
        )));
    };

    let source = onchain_code_source(version_key, version.availability)?;
    let hash = CryptoHash(version.code_hash.into());

    resolve_available_version(
        ctx,
        &client,
        version_key,
        source,
        hash,
        catalogued_code(hash.0).await,
    )
    .await
}

fn onchain_code_source(
    version_key: &str,
    availability: VersionAvailability,
) -> GatewayResult<OnchainCodeSource> {
    match availability {
        VersionAvailability::Global => Ok(OnchainCodeSource::Global),
        VersionAvailability::Stored { code_len } if code_len <= MAX_STORED_CODE_LEN => {
            Ok(OnchainCodeSource::Stored(StoredCodeLen(code_len)))
        }
        VersionAvailability::Stored { .. } => Err(precondition(format!(
            "registry version {version_key} reports code larger than {MAX_STORED_CODE_LEN} bytes"
        ))),
        VersionAvailability::Removed => Err(precondition(format!(
            "registry version {version_key} has been removed and cannot be deployed"
        ))),
    }
}

async fn resolve_available_version<C: HasNearClient>(
    ctx: &C,
    client: &templar_gateway_core::client::registry::RegistryClient<'_>,
    version_key: &str,
    source: OnchainCodeSource,
    hash: CryptoHash,
    catalogued: Option<Vec<u8>>,
) -> GatewayResult<Vec<u8>> {
    if let Some(code) = catalogued {
        return Ok(code);
    }

    let code = match source {
        OnchainCodeSource::Global => ctx.near_client().view_global_contract_code(hash).await?,
        OnchainCodeSource::Stored(code_len) => {
            read_stored_code(client, version_key, code_len).await?
        }
    };

    verify_code_hash(code, hash)
}

async fn resolve_legacy_registry_wasm<C: HasNearClient>(
    ctx: &C,
    client: &templar_gateway_core::client::registry::RegistryClient<'_>,
    version_key: &str,
) -> GatewayResult<Vec<u8>> {
    let Some(hash) = client
        .get_version_code_hash(GetVersionArgs {
            version_key: version_key.to_owned(),
        })
        .await?
    else {
        return Err(precondition(format!(
            "registry version {version_key} does not exist"
        )));
    };

    let hash = CryptoHash(hash.into());

    let code = ctx
        .near_client()
        .view_global_contract_code(hash)
        .await
        .map_err(|error| match error {
            GatewayError::GlobalContractCodeNotFound(_) => precondition(format!(
                "registry version {version_key} is not global and this registry cannot expose stored code; upgrade the registry before deployment"
            )),
            other => other,
        })?;

    verify_code_hash(code, hash)
}

async fn catalogued_code(hash: [u8; 32]) -> Option<Vec<u8>> {
    fetch::released_bytes_by_sha256(&hash).await.ok()
}

async fn read_stored_code(
    client: &templar_gateway_core::client::registry::RegistryClient<'_>,
    version_key: &str,
    code_len: StoredCodeLen,
) -> GatewayResult<Vec<u8>> {
    let mut code = Vec::with_capacity(code_len.0 as usize);
    let mut request = GetVersionCodeChunkArgs {
        version_key: version_key.to_owned(),
        offset: 0,
        len: 0,
    };

    while request.offset < code_len.0 {
        request.len = (code_len.0 - request.offset).min(CODE_CHUNK_LEN);
        let Some(chunk) = client.get_version_code_chunk(&request).await? else {
            return Err(precondition(format!(
                "registry version {version_key} no longer has stored code"
            )));
        };

        if chunk.is_empty() || chunk.len() > request.len as usize {
            return Err(precondition(format!(
                "registry version {version_key} returned an invalid code chunk"
            )));
        }

        request.offset += u32::try_from(chunk.len()).map_err(|_| {
            precondition(format!(
                "registry version {version_key} returned an invalid code chunk"
            ))
        })?;
        code.extend(chunk);
    }

    Ok(code)
}

fn verify_code_hash(code: Vec<u8>, expected: CryptoHash) -> GatewayResult<Vec<u8>> {
    let actual = CryptoHash(Sha256::digest(&code).into());
    if actual != expected {
        return Err(precondition(format!(
            "resolved registry code hash {actual} does not match expected {expected}"
        )));
    }

    Ok(code)
}

fn precondition(message: String) -> GatewayError {
    GatewayError::RequestPreconditionFailed(message)
}

#[cfg(test)]
mod tests {
    use super::{
        onchain_code_source, resolve_available_version, verify_code_hash, CryptoHash,
        OnchainCodeSource, Sha256, StoredCodeLen, MAX_STORED_CODE_LEN,
    };
    use crate::test_ctx::offline_ctx;
    use sha2::Digest;
    use templar_common::registry::VersionAvailability;
    use templar_gateway_core::{GatewayError, HasNearClient};

    #[test]
    fn rejects_resolved_code_with_a_different_hash() {
        let error =
            verify_code_hash(vec![1, 2, 3], CryptoHash([0; 32])).expect_err("mismatched code hash");

        assert!(error.to_string().contains("does not match"));
    }

    #[test]
    fn rejects_an_oversized_stored_code_advertisement() {
        let error = onchain_code_source(
            "oversized@1.0.0",
            VersionAvailability::Stored {
                code_len: MAX_STORED_CODE_LEN + 1,
            },
        )
        .expect_err("stored code must respect the registry storage limit");

        assert!(error.to_string().contains("reports code larger"));
    }

    #[tokio::test]
    async fn catalogued_normal_code_skips_the_chunk_query() {
        let ctx = offline_ctx();
        let code = vec![1, 2, 3];
        let hash = CryptoHash(Sha256::digest(&code).into());
        let client = ctx
            .near_client()
            .registry("registry.near".parse().expect("valid account ID"));

        let resolved = resolve_available_version(
            &ctx,
            &client,
            "catalogued@1.0.0",
            OnchainCodeSource::Stored(StoredCodeLen(300)),
            hash,
            Some(code.clone()),
        )
        .await
        .expect("catalogued code must not use the unreachable chunk query");

        assert_eq!(resolved, code);
    }

    #[tokio::test]
    async fn unavailable_catalog_uses_the_onchain_source() {
        let ctx = offline_ctx();
        let client = ctx
            .near_client()
            .registry("registry.near".parse().expect("valid account ID"));
        let error = resolve_available_version(
            &ctx,
            &client,
            "uncatalogued@1.0.0",
            OnchainCodeSource::Stored(StoredCodeLen(1)),
            CryptoHash([0; 32]),
            None,
        )
        .await
        .expect_err("an unavailable catalog must continue to the onchain source");

        assert!(matches!(error, GatewayError::NearQuery(_)));
    }
}
