use crate::{strnum::SU64, *};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(transparent))]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[cfg_attr(
    feature = "borsh",
    derive(borsh::BorshSerialize, borsh::BorshDeserialize, borsh::BorshSchema)
)]
pub struct Nanoseconds(SU64);

impl Nanoseconds {
    pub const fn zero() -> Self {
        Self(SU64::new(0))
    }

    /// Creates a `Nanoseconds` value from milliseconds.
    pub const fn from_ns(value: u64) -> Self {
        Self(SU64::new(value))
    }

    /// Creates a `Nanoseconds` value from milliseconds.
    pub const fn from_ms(value: u64) -> Self {
        Self(SU64::new(value.saturating_mul(1_000_000)))
    }

    /// Creates a `Milliseconds` value from seconds.
    pub const fn from_secs(value: u64) -> Self {
        Self(SU64::new(value.saturating_mul(1_000_000_000)))
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

    #[must_use]
    pub const fn saturating_add(self, rhs: Self) -> Self {
        Self(SU64::new(self.0 .0.saturating_add(rhs.0 .0)))
    }

    #[must_use]
    pub const fn saturating_sub(self, rhs: Self) -> Self {
        Self(SU64::new(self.0 .0.saturating_sub(rhs.0 .0)))
    }
}

impl std::fmt::Display for Nanoseconds {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}ns", self.as_ns())
    }
}

#[cfg(feature = "near")]
mod near {
    impl super::Nanoseconds {
        pub fn near_timestamp() -> Self {
            Self::from_ns(near_sdk::env::block_timestamp())
        }
    }
}

#[cfg(feature = "redstone")]
mod redstone {
    impl From<redstone::TimestampMillis> for super::Nanoseconds {
        fn from(value: redstone::TimestampMillis) -> Self {
            Self::from_ms(value.as_millis())
        }
    }

    impl From<super::Nanoseconds> for redstone::TimestampMillis {
        fn from(value: super::Nanoseconds) -> Self {
            Self::from_millis(value.as_ms())
        }
    }
}
