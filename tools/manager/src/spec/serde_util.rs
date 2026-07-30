//! Serde adapters letting the spec use the readable form of types whose
//! on-chain encoding is not meant to be hand-written.

use serde::{Deserialize, Deserializer, Serializer};
use templar_common::{
    asset::{AssetClass, FungibleAsset},
    Nanoseconds,
};

use crate::commands::duration::parse_duration;

/// `FungibleAsset` as `nep141:<contract>` / `nep245:<contract>:<token>`, via the
/// `Display`/`FromStr` pair it already has, instead of the externally-tagged
/// `{"Nep245": {"contract_id": …, "token_id": …}}` object.
pub(crate) mod fungible_asset {
    use super::{AssetClass, Deserialize, Deserializer, FungibleAsset, Serializer};

    pub(crate) fn serialize<S, A>(
        asset: &FungibleAsset<A>,
        serializer: S,
    ) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
        A: AssetClass,
    {
        serializer.collect_str(asset)
    }

    pub(crate) fn deserialize<'de, D, A>(deserializer: D) -> Result<FungibleAsset<A>, D::Error>
    where
        D: Deserializer<'de>,
        A: AssetClass,
    {
        let raw = String::deserialize(deserializer)?;
        raw.parse().map_err(serde::de::Error::custom)
    }
}

/// A duration as a unit-suffixed string (`10m`, `60s`), parsed by the same
/// [`parse_duration`] the time flags use, so the spec and the CLI cannot drift
/// on what `1h` means.
pub(crate) mod duration {
    use super::{parse_duration, Deserialize, Deserializer, Nanoseconds, Serializer};

    #[allow(
        clippy::trivially_copy_pass_by_ref,
        reason = "serde's `serialize_with` fixes this signature"
    )]
    pub(crate) fn serialize<S>(value: &Nanoseconds, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        // `ns` round-trips exactly. Emitting a coarser unit would need to prove
        // the value divides evenly, and nothing reads this back but the parser.
        serializer.collect_str(&format_args!("{}ns", value.as_ns()))
    }

    pub(crate) fn deserialize<'de, D>(deserializer: D) -> Result<Nanoseconds, D::Error>
    where
        D: Deserializer<'de>,
    {
        let raw = String::deserialize(deserializer)?;
        parse_duration(&raw).map_err(serde::de::Error::custom)
    }
}

/// [`duration`], for an optional field. Serialization delegates so the emitted
/// form cannot drift between the two.
pub(crate) mod duration_opt {
    use super::{duration, parse_duration, Deserialize, Deserializer, Nanoseconds, Serializer};

    #[allow(
        clippy::ref_option,
        reason = "serde's `serialize_with` fixes this signature"
    )]
    pub(crate) fn serialize<S>(
        value: &Option<Nanoseconds>,
        serializer: S,
    ) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match value {
            Some(value) => duration::serialize(value, serializer),
            None => serializer.serialize_none(),
        }
    }

    pub(crate) fn deserialize<'de, D>(deserializer: D) -> Result<Option<Nanoseconds>, D::Error>
    where
        D: Deserializer<'de>,
    {
        Option::<String>::deserialize(deserializer)?
            .map(|raw| parse_duration(&raw))
            .transpose()
            .map_err(serde::de::Error::custom)
    }
}
