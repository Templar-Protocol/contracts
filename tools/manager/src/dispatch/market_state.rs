//! The envelope proven by fixtures/market-layouts/market-1.6.1.json.
//! Only the prefix length and configuration are decoded; the rest is opaque.

use anyhow::{ensure, Context, Result};
use borsh::BorshDeserialize as _;
use templar_common::market::MarketConfiguration;
use templar_contract_artifacts::{release_by_sha256, ArtifactId, ArtifactRelease};

pub(crate) const MARKET_1_6_1_SHA256: &str =
    "d75c2419527d816a8f5fcab6a585032799a38cf90e09a5a98992d5c84774562a";
pub(crate) const MARKET_1_6_1_LENGTH: usize = 520_372;
pub(crate) const LAYOUT: &str = "market-1.6.1-prefix-configuration";

/// Release recognition and layout admission are deliberately separate checks.
/// Catalog membership alone never admits a new layout.
pub(crate) fn supported_release(code: &[u8]) -> Result<&'static ArtifactRelease> {
    use sha2::{Digest as _, Sha256};
    let digest: [u8; 32] = Sha256::digest(code).into();
    let (artifact, release) = release_by_sha256(&digest).with_context(|| {
        format!(
            "unsupported market WASM {}; add catalog identity and nonempty deployed STATE layout evidence before authoring patches",
            hex::encode(digest)
        )
    })?;
    ensure!(
        artifact == ArtifactId::Market
            && release.version == "1.6.1"
            && release.sha256 == MARKET_1_6_1_SHA256
            && release.length == MARKET_1_6_1_LENGTH
            && code.len() == MARKET_1_6_1_LENGTH,
        "unsupported market layout for {}@{}; only fixture-proven Market 1.6.1 is admitted",
        artifact,
        release.version
    );
    Ok(release)
}

pub(crate) struct ConfigurationEnvelope {
    pub(crate) configuration: MarketConfiguration,
    pub(crate) configuration_start: usize,
    pub(crate) configuration_end: usize,
}

/// Bound the prefix before deserializing, avoiding allocation from an untrusted
/// Vec length. Borsh consumes only the configuration, not the opaque suffix.
pub(crate) fn decode(state: &[u8]) -> Result<ConfigurationEnvelope> {
    let prefix_length = u32::from_le_bytes(
        state
            .get(..4)
            .context("truncated STATE prefix length")?
            .try_into()?,
    );
    let configuration_start = 4usize
        .checked_add(usize::try_from(prefix_length)?)
        .context("STATE prefix length overflow")?;
    let mut remaining = state
        .get(configuration_start..)
        .context("truncated STATE prefix")?;
    let configuration = MarketConfiguration::deserialize(&mut remaining)
        .context("decode STATE market configuration")?;
    let configuration_end = state.len() - remaining.len();
    ensure!(
        !remaining.is_empty(),
        "truncated STATE: missing opaque contract suffix"
    );
    ensure!(
        borsh::to_vec(&configuration)? == state[configuration_start..configuration_end],
        "STATE configuration is not canonical round-trippable Borsh"
    );
    configuration
        .validate()
        .context("validate STATE configuration")?;
    Ok(ConfigurationEnvelope {
        configuration,
        configuration_start,
        configuration_end,
    })
}

impl ConfigurationEnvelope {
    pub(crate) fn replace(
        &self,
        state: &[u8],
        configuration: &MarketConfiguration,
    ) -> Result<Vec<u8>> {
        let encoded = borsh::to_vec(configuration).context("encode replacement configuration")?;
        let mut replacement = Vec::with_capacity(
            self.configuration_start + encoded.len() + state.len() - self.configuration_end,
        );
        replacement.extend_from_slice(&state[..self.configuration_start]);
        replacement.extend_from_slice(&encoded);
        replacement.extend_from_slice(&state[self.configuration_end..]);
        Ok(replacement)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_truncated_and_overlong_prefixes() {
        for state in [vec![], vec![0], vec![0; 3], vec![1, 0, 0, 0], vec![255; 4]] {
            assert!(decode(&state).is_err());
        }
    }

    #[test]
    fn rejects_truncated_configuration() {
        for state in [vec![0; 4], vec![0; 5], vec![1, 0, 0, 0, 42, 0]] {
            assert!(decode(&state).is_err());
        }
    }
}
