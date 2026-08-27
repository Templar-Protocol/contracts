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
struct StoredCodeLen {
    value: u32,
}

impl TryFrom<u32> for StoredCodeLen {
    type Error = ();

    fn try_from(value: u32) -> Result<Self, Self::Error> {
        (value <= MAX_STORED_CODE_LEN)
            .then_some(Self { value })
            .ok_or(())
    }
}

impl StoredCodeLen {
    fn value(self) -> u32 {
        self.value
    }
}

struct StoredCodeAssembler {
    expected_len: StoredCodeLen,
    offset: u32,
    bytes: Vec<u8>,
}

impl StoredCodeAssembler {
    fn new(expected_len: StoredCodeLen) -> Self {
        Self {
            expected_len,
            offset: 0,
            bytes: Vec::with_capacity(expected_len.value() as usize),
        }
    }

    fn next_len(&self) -> Option<u32> {
        (self.offset < self.expected_len.value())
            .then(|| (self.expected_len.value() - self.offset).min(CODE_CHUNK_LEN))
    }

    fn push(
        &mut self,
        version_key: &str,
        requested_len: u32,
        chunk: Option<Vec<u8>>,
    ) -> GatewayResult<()> {
        let invalid = || {
            precondition(format!(
                "registry version {version_key} returned an invalid code chunk"
            ))
        };
        let chunk = chunk.ok_or_else(invalid)?;
        let chunk_len = u32::try_from(chunk.len()).map_err(|_| invalid())?;
        let remaining = self
            .expected_len
            .value()
            .checked_sub(self.offset)
            .ok_or_else(invalid)?;
        if chunk_len == 0
            || chunk_len > requested_len
            || requested_len > remaining
            || self
                .offset
                .checked_add(chunk_len)
                .is_none_or(|end| end > self.expected_len.value())
        {
            return Err(invalid());
        }

        self.offset += chunk_len;
        self.bytes.extend(chunk);
        Ok(())
    }

    fn finish(self) -> GatewayResult<Vec<u8>> {
        if self.next_len().is_some() {
            return Err(precondition(
                "stored code assembly ended before the advertised length".to_owned(),
            ));
        }
        Ok(self.bytes)
    }
}

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
        VersionAvailability::Stored { code_len } => {
            let code_len = StoredCodeLen::try_from(code_len).map_err(|()| {
                precondition(format!(
                    "registry version {version_key} reports code larger than {MAX_STORED_CODE_LEN} bytes"
                ))
            })?;
            Ok(OnchainCodeSource::Stored(code_len))
        }
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

    resolve_available_version(
        ctx,
        client,
        version_key,
        OnchainCodeSource::Global,
        hash,
        catalogued_code(hash.0).await,
    )
    .await
    .map_err(|error| match error {
        GatewayError::GlobalContractCodeNotFound(_) => precondition(format!(
            "registry version {version_key} is not global and this registry cannot expose stored code; upgrade the registry before deployment"
        )),
        other => other,
    })
}

async fn catalogued_code(hash: [u8; 32]) -> Option<Vec<u8>> {
    fetch::released_bytes_by_sha256(&hash).await.ok()
}

async fn read_stored_code(
    client: &templar_gateway_core::client::registry::RegistryClient<'_>,
    version_key: &str,
    code_len: StoredCodeLen,
) -> GatewayResult<Vec<u8>> {
    let mut assembler = StoredCodeAssembler::new(code_len);
    while let Some(len) = assembler.next_len() {
        let request = GetVersionCodeChunkArgs {
            version_key: version_key.to_owned(),
            offset: assembler.offset,
            len,
        };
        let chunk = client.get_version_code_chunk(&request).await?;
        if chunk.is_none() {
            return Err(precondition(format!(
                "registry version {version_key} no longer has stored code"
            )));
        }
        assembler.push(version_key, len, chunk)?;
    }

    assembler.finish()
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
        OnchainCodeSource, Sha256, StoredCodeAssembler, StoredCodeLen, CODE_CHUNK_LEN,
        MAX_STORED_CODE_LEN,
    };
    use crate::test_ctx::offline_ctx;
    use sha2::Digest;
    use templar_common::registry::VersionAvailability;
    use templar_gateway_core::{GatewayError, HasNearClient};

    fn stored_len(value: u32) -> StoredCodeLen {
        StoredCodeLen::try_from(value).expect("valid stored code length")
    }

    #[test]
    fn accepts_maximum_stored_code_length_and_rejects_the_next_value() {
        assert!(StoredCodeLen::try_from(MAX_STORED_CODE_LEN).is_ok());
        assert!(StoredCodeLen::try_from(MAX_STORED_CODE_LEN + 1).is_err());
    }

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

    #[test]
    fn assembles_chunks_with_bounded_progression() {
        let mut assembler = StoredCodeAssembler::new(stored_len(CODE_CHUNK_LEN + 3));
        assert_eq!(assembler.next_len(), Some(CODE_CHUNK_LEN));
        assembler
            .push(
                "registry.near",
                CODE_CHUNK_LEN,
                Some(vec![1; CODE_CHUNK_LEN as usize]),
            )
            .expect("first chunk");
        assert_eq!(assembler.next_len(), Some(3));
        assembler
            .push("registry.near", 3, Some(vec![2; 3]))
            .expect("final chunk");
        assert_eq!(assembler.next_len(), None);
    }
    #[test]
    fn finishes_after_final_chunk() {
        let mut assembler = StoredCodeAssembler::new(stored_len(3));
        assembler
            .push("registry.near", 3, Some(vec![1, 2, 3]))
            .expect("final chunk");

        assert_eq!(assembler.next_len(), None);
        assert_eq!(
            assembler.finish().expect("complete assembly"),
            vec![1, 2, 3]
        );
    }

    #[test]
    fn rejects_missing_empty_oversized_and_overrun_chunks_without_progress() {
        let mut assembler = StoredCodeAssembler::new(stored_len(3));
        for (requested_len, chunk) in [
            (3, None),
            (3, Some(Vec::new())),
            (2, Some(vec![1, 2, 3])),
            (4, Some(vec![1, 2, 3])),
        ] {
            assembler
                .push("registry.near", requested_len, chunk)
                .expect_err("malformed chunk");
            assert_eq!(assembler.next_len(), Some(3));
        }
    }

    #[tokio::test]
    async fn catalogued_code_skips_both_onchain_sources() {
        let ctx = offline_ctx();
        let code = vec![1, 2, 3];
        let hash = CryptoHash(Sha256::digest(&code).into());
        let client = ctx
            .near_client()
            .registry("registry.near".parse().expect("valid account ID"));

        let stored = resolve_available_version(
            &ctx,
            &client,
            "catalogued@1.0.0",
            OnchainCodeSource::Stored(stored_len(300)),
            hash,
            Some(code.clone()),
        )
        .await
        .expect("catalogued stored code must not query the chain");
        let global = resolve_available_version(
            &ctx,
            &client,
            "catalogued@1.0.0",
            OnchainCodeSource::Global,
            hash,
            Some(code.clone()),
        )
        .await
        .expect("catalogued global code must not query the chain");

        assert_eq!(stored, code);
        assert_eq!(global, code);
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
            OnchainCodeSource::Stored(stored_len(1)),
            CryptoHash([0; 32]),
            None,
        )
        .await
        .expect_err("an unavailable catalog must continue to the onchain source");

        assert!(matches!(error, GatewayError::NearQuery(_)));
    }
}
