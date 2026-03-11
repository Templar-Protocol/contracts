mod registry_version;
pub use registry_version::{Registry, RegistryVersion};
mod market_version;
pub use market_version::{Market, MarketVersion};

type N = u64;
type Repr = (N, N, N);

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Version<T> {
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
