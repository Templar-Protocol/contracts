use std::ops::{Add, AddAssign, Sub, SubAssign};

use near_sdk::{json_types::U64, near};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[near(serializers = [json, borsh])]
#[serde(transparent)]
pub struct Milliseconds(U64);

impl Milliseconds {
    pub const fn zero() -> Self {
        Self(U64(0))
    }

    /// Creates a `Milliseconds` value from milliseconds.
    pub const fn from_ms(value: u64) -> Self {
        Self(U64(value))
    }

    /// Creates a `Milliseconds` value from seconds.
    pub const fn from_s(value: u64) -> Self {
        Self(U64(value.saturating_mul(1000)))
    }

    /// Returns the value as seconds, truncated.
    pub const fn as_s(&self) -> u64 {
        self.0 .0 / 1000
    }

    /// Returns the value as milliseconds.
    pub const fn as_ms(&self) -> u64 {
        self.0 .0
    }

    /// Creates a `Milliseconds` value from seconds, returning `None` if the value is negative.
    pub fn try_from_secs_i64(value: i64) -> Option<Self> {
        let ms = value.checked_mul(1000)?;
        Some(Self(U64(u64::try_from(ms).ok()?)))
    }

    pub fn now() -> Self {
        Self::from_ms(near_sdk::env::block_timestamp_ms())
    }
}

impl std::fmt::Display for Milliseconds {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}ms", self.as_ms())
    }
}

impl From<redstone::TimestampMillis> for Milliseconds {
    fn from(value: redstone::TimestampMillis) -> Self {
        Self(U64(value.as_millis()))
    }
}

impl From<Milliseconds> for redstone::TimestampMillis {
    fn from(value: Milliseconds) -> Self {
        Self::from_millis(value.as_ms())
    }
}

impl From<Milliseconds> for u64 {
    fn from(value: Milliseconds) -> Self {
        value.0.into()
    }
}

impl Add for Milliseconds {
    type Output = Self;

    fn add(self, rhs: Self) -> Self::Output {
        Self(U64(self.0 .0 + rhs.0 .0))
    }
}

impl AddAssign for Milliseconds {
    fn add_assign(&mut self, rhs: Self) {
        self.0 .0 += rhs.0 .0;
    }
}

impl Sub for Milliseconds {
    type Output = Self;

    fn sub(self, rhs: Self) -> Self::Output {
        Self(U64(self.0 .0 - rhs.0 .0))
    }
}

impl SubAssign for Milliseconds {
    fn sub_assign(&mut self, rhs: Self) {
        self.0 .0 -= rhs.0 .0;
    }
}
