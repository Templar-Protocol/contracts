#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MarketVersion {
    pub major: u16,
    pub minor: u16,
    pub patch: u16,
}

impl std::fmt::Display for MarketVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

impl From<MarketVersion> for (u16, u16, u16) {
    fn from(value: MarketVersion) -> Self {
        (value.major, value.minor, value.patch)
    }
}

impl From<(u16, u16, u16)> for MarketVersion {
    fn from((major, minor, patch): (u16, u16, u16)) -> Self {
        Self {
            major,
            minor,
            patch,
        }
    }
}

impl std::cmp::PartialEq<(u16, u16, u16)> for MarketVersion {
    fn eq(&self, other: &(u16, u16, u16)) -> bool {
        <(u16, u16, u16)>::from(*self).eq(other)
    }
}

impl std::cmp::PartialOrd<(u16, u16, u16)> for MarketVersion {
    fn partial_cmp(&self, other: &(u16, u16, u16)) -> Option<std::cmp::Ordering> {
        <(u16, u16, u16)>::from(*self).partial_cmp(other)
    }
}

impl MarketVersion {
    pub fn supports_partial_liquidation(self) -> bool {
        self >= (1, 1, 0)
    }

    pub fn requires_static_yield_accumulation(self) -> bool {
        self >= (1, 1, 0)
    }
}

#[derive(thiserror::Error, Debug, Clone, PartialEq, Eq)]
pub enum ParseError {
    #[error("Missing separator index {0}")]
    Separator(usize),
    #[error("Failed to parse segment index {0}")]
    Segment(usize),
}

impl std::str::FromStr for MarketVersion {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (major, tail) = s.split_once('.').ok_or(ParseError::Separator(0))?;
        let major: u16 = major.parse().map_err(|_| ParseError::Segment(0))?;
        let (minor, patch) = tail.split_once('.').ok_or(ParseError::Separator(1))?;
        let minor: u16 = minor.parse().map_err(|_| ParseError::Segment(1))?;
        let patch: u16 = patch.parse().map_err(|_| ParseError::Segment(2))?;
        Ok(Self {
            major,
            minor,
            patch,
        })
    }
}
