use std::ops::{Add, AddAssign, Sub, SubAssign};

use near_sdk::{json_types::U64, near};

use crate::oracle::pyth::PythTimestamp;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[near(serializers = [json, borsh])]
#[serde(transparent)]
pub struct Nanoseconds(U64);

impl Nanoseconds {
    pub fn try_from_pyth(value: PythTimestamp) -> Option<Self> {
        let ms = value.as_ms()?;
        Some(Self::from_ms(u64::try_from(ms).ok()?))
    }

    pub fn try_to_pyth(&self) -> Option<PythTimestamp> {
        Some(PythTimestamp::from_ms(i64::try_from(self.as_ms()).ok()?))
    }

    pub const fn zero() -> Self {
        Self(U64(0))
    }

    /// Creates a `Nanoseconds` value from milliseconds.
    pub const fn from_ns(value: u64) -> Self {
        Self(U64(value))
    }

    /// Creates a `Nanoseconds` value from milliseconds.
    pub const fn from_ms(value: u64) -> Self {
        Self(U64(value.saturating_mul(1_000_000)))
    }

    /// Creates a `Milliseconds` value from seconds.
    pub const fn from_secs(value: u64) -> Self {
        Self(U64(value.saturating_mul(1_000_000_000)))
    }

    /// Returns the value as seconds, truncated.
    pub const fn as_secs(&self) -> u64 {
        self.0 .0 / 1_000_000_000
    }

    /// Returns the value as milliseconds, truncated.
    pub const fn as_ms(&self) -> u64 {
        self.0 .0 / 1_000_000
    }

    /// Returns the value as nanoseconds.
    pub const fn as_ns(&self) -> u64 {
        self.0 .0
    }

    pub fn now() -> Self {
        Self::from_ns(near_sdk::env::block_timestamp())
    }

    #[must_use]
    pub const fn saturating_add(self, rhs: Self) -> Self {
        Self(U64(self.0 .0.saturating_add(rhs.0 .0)))
    }

    #[must_use]
    pub const fn saturating_sub(self, rhs: Self) -> Self {
        Self(U64(self.0 .0.saturating_sub(rhs.0 .0)))
    }
}

impl std::fmt::Display for Nanoseconds {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}ns", self.as_ns())
    }
}

impl From<redstone::TimestampMillis> for Nanoseconds {
    fn from(value: redstone::TimestampMillis) -> Self {
        Self::from_ms(value.as_millis())
    }
}

impl From<Nanoseconds> for redstone::TimestampMillis {
    fn from(value: Nanoseconds) -> Self {
        Self::from_millis(value.as_ms())
    }
}

impl From<Nanoseconds> for u64 {
    fn from(value: Nanoseconds) -> Self {
        value.0.into()
    }
}

impl Add for Nanoseconds {
    type Output = Self;

    fn add(self, rhs: Self) -> Self::Output {
        Self(U64(self.0 .0 + rhs.0 .0))
    }
}

impl AddAssign for Nanoseconds {
    fn add_assign(&mut self, rhs: Self) {
        self.0 .0 += rhs.0 .0;
    }
}

impl Sub for Nanoseconds {
    type Output = Self;

    fn sub(self, rhs: Self) -> Self::Output {
        Self(U64(self.0 .0 - rhs.0 .0))
    }
}

impl SubAssign for Nanoseconds {
    fn sub_assign(&mut self, rhs: Self) {
        self.0 .0 -= rhs.0 .0;
    }
}
