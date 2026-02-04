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

impl std::str::FromStr for MarketVersion {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match &s.split('.').flat_map(u16::from_str).collect::<Vec<_>>()[..] {
            [major, minor, patch] => Ok(Self {
                major: *major,
                minor: *minor,
                patch: *patch,
            }),
            _ => anyhow::bail!("Failed to parse market version string \"{s}\""),
        }
    }
}
