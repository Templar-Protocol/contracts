mod market_version;
mod proxy_oracle_version;
mod registry_version;

pub use market_version::{Market, MarketVersion};
pub use proxy_oracle_version::{ProxyOracle, ProxyOracleVersion};
pub use registry_version::{Registry, RegistryVersion};

type N = u64;
type Repr = (N, N, N);

#[derive(
    Debug,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    serde::Serialize,
    serde::Deserialize,
    schemars::JsonSchema,
)]
pub struct Version<T> {
    #[serde(skip)]
    #[schemars(skip)]
    _phantom: std::marker::PhantomData<T>,
    pub major: N,
    pub minor: N,
    pub patch: N,
}

impl<T> Clone for Version<T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T> Copy for Version<T> {}

impl<T> Version<T> {
    /// Reinterpret this version under a different contract-kind tag.
    ///
    /// The numeric version is unchanged; only the phantom marker differs. This
    /// is the consumer-side assertion "I know which contract this version came
    /// from" — e.g. turning the kind-agnostic `Version<()>` that
    /// `contract.getVersion` returns into a [`MarketVersion`].
    #[must_use]
    pub fn cast<U>(self) -> Version<U> {
        Version {
            _phantom: std::marker::PhantomData,
            major: self.major,
            minor: self.minor,
            patch: self.patch,
        }
    }
}

impl<T> std::fmt::Display for Version<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

impl<T> From<Version<T>> for Repr {
    fn from(value: Version<T>) -> Self {
        (value.major, value.minor, value.patch)
    }
}

impl<T> From<&Version<T>> for Repr {
    fn from(value: &Version<T>) -> Self {
        (value.major, value.minor, value.patch)
    }
}

impl<T> From<Repr> for Version<T> {
    fn from((major, minor, patch): Repr) -> Self {
        Self {
            _phantom: std::marker::PhantomData,
            major,
            minor,
            patch,
        }
    }
}

impl<T> std::cmp::PartialEq<Repr> for Version<T> {
    fn eq(&self, other: &Repr) -> bool {
        <Repr>::from(self).eq(other)
    }
}

impl<T> std::cmp::PartialOrd<Repr> for Version<T> {
    fn partial_cmp(&self, other: &Repr) -> Option<std::cmp::Ordering> {
        <Repr>::from(self).partial_cmp(other)
    }
}

#[derive(thiserror::Error, Debug, Clone, PartialEq, Eq)]
pub enum ParseError {
    #[error("Missing separator index {index} in input '{input}'")]
    Separator { index: usize, input: String },
    #[error("Failed to parse segment index {index} in input '{input}'")]
    Segment { index: usize, input: String },
}

impl<T> std::str::FromStr for Version<T> {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (major, tail) = s.split_once('.').ok_or(ParseError::Separator {
            index: 0,
            input: s.to_string(),
        })?;
        let major: N = major.parse().map_err(|_| ParseError::Segment {
            index: 0,
            input: s.to_string(),
        })?;
        let (minor, patch) = tail.split_once('.').ok_or(ParseError::Separator {
            index: 1,
            input: s.to_string(),
        })?;
        let minor: N = minor.parse().map_err(|_| ParseError::Segment {
            index: 1,
            input: s.to_string(),
        })?;
        let patch: N = patch.parse().map_err(|_| ParseError::Segment {
            index: 2,
            input: s.to_string(),
        })?;

        Ok(Self {
            _phantom: std::marker::PhantomData,
            major,
            minor,
            patch,
        })
    }
}
