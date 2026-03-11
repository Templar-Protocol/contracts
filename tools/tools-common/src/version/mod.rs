mod registry_version;
pub use registry_version::RegistryVersion;
mod market_version;
pub use market_version::MarketVersion;

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Version<T> {
    _phantom: std::marker::PhantomData<T>,
    pub major: u16,
    pub minor: u16,
    pub patch: u16,
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

impl<T> From<Version<T>> for (u16, u16, u16) {
    fn from(value: Version<T>) -> Self {
        (value.major, value.minor, value.patch)
    }
}

impl<T> From<&Version<T>> for (u16, u16, u16) {
    fn from(value: &Version<T>) -> Self {
        (value.major, value.minor, value.patch)
    }
}

impl<T> From<(u16, u16, u16)> for Version<T> {
    fn from((major, minor, patch): (u16, u16, u16)) -> Self {
        Self {
            _phantom: std::marker::PhantomData,
            major,
            minor,
            patch,
        }
    }
}

impl<T> std::cmp::PartialEq<(u16, u16, u16)> for Version<T> {
    fn eq(&self, other: &(u16, u16, u16)) -> bool {
        <(u16, u16, u16)>::from(self).eq(other)
    }
}

impl<T> std::cmp::PartialOrd<(u16, u16, u16)> for Version<T> {
    fn partial_cmp(&self, other: &(u16, u16, u16)) -> Option<std::cmp::Ordering> {
        <(u16, u16, u16)>::from(self).partial_cmp(other)
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
        let major: u16 = major.parse().map_err(|_| ParseError::Segment {
            index: 0,
            input: s.to_string(),
        })?;
        let (minor, patch) = tail.split_once('.').ok_or(ParseError::Separator {
            index: 1,
            input: s.to_string(),
        })?;
        let minor: u16 = minor.parse().map_err(|_| ParseError::Segment {
            index: 1,
            input: s.to_string(),
        })?;
        let patch: u16 = patch.parse().map_err(|_| ParseError::Segment {
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
