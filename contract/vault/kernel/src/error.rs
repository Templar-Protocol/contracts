//! Kernel error types.

use crate::restrictions::RestrictionKind;
use crate::transitions::TransitionError;

/// Errors that can occur when applying kernel actions.
#[cfg_attr(not(target_arch = "wasm32"), derive(Debug))]
#[derive(Clone, PartialEq, Eq)]
pub enum KernelError {
    #[cfg(target_arch = "wasm32")]
    InvalidState,
    #[cfg(not(target_arch = "wasm32"))]
    InvalidState(&'static str),
    OpIdMismatch {
        expected: u64,
        actual: u64,
    },
    Slippage {
        min: u128,
        actual: u128,
    },
    MinWithdrawal {
        amount: u128,
        min: u128,
    },
    QueueFull {
        current: u32,
        max: u32,
    },
    EmptyQueue,
    Cooldown {
        requested_at: u64,
        now: u64,
        cooldown_ns: u64,
    },
    Transition(TransitionError),
    NotImplemented,
    Restricted(RestrictionKind),
    #[cfg(target_arch = "wasm32")]
    InvalidConfig,
    #[cfg(not(target_arch = "wasm32"))]
    InvalidConfig(&'static str),
    ZeroAmount,
}

impl KernelError {
    #[inline]
    #[must_use]
    pub const fn invalid_state(message: &'static str) -> Self {
        #[cfg(target_arch = "wasm32")]
        {
            let _ = message;
            Self::InvalidState
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            Self::InvalidState(message)
        }
    }

    #[inline]
    #[must_use]
    pub const fn invalid_config(message: &'static str) -> Self {
        #[cfg(target_arch = "wasm32")]
        {
            let _ = message;
            Self::InvalidConfig
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            Self::InvalidConfig(message)
        }
    }

    /// Stable numeric code for on-chain debugging and indexing.
    #[must_use]
    pub fn code(&self) -> u32 {
        match self {
            #[cfg(target_arch = "wasm32")]
            KernelError::InvalidState => 1000,
            #[cfg(not(target_arch = "wasm32"))]
            KernelError::InvalidState(_) => 1000,
            KernelError::OpIdMismatch { .. } => 1001,
            KernelError::Slippage { .. } => 1002,
            KernelError::MinWithdrawal { .. } => 1003,
            KernelError::QueueFull { .. } => 1004,
            KernelError::EmptyQueue => 1005,
            KernelError::Cooldown { .. } => 1006,
            KernelError::Transition(_) => 1007,
            KernelError::NotImplemented => 1008,
            KernelError::Restricted(_) => 1009,
            #[cfg(target_arch = "wasm32")]
            KernelError::InvalidConfig => 1010,
            #[cfg(not(target_arch = "wasm32"))]
            KernelError::InvalidConfig(_) => 1010,
            KernelError::ZeroAmount => 1011,
        }
    }
}
