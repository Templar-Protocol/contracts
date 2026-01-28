//! Runtime error types.

use alloc::string::String;

/// Errors that can occur during runtime execution.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RuntimeError {
    /// Authorization failed.
    Unauthorized(String),
    /// Insufficient balance for the operation.
    InsufficientBalance {
        available: u128,
        required: u128,
    },
    /// Invalid operation state.
    InvalidState(String),
    /// Storage error.
    StorageError(String),
    /// Effect execution failed.
    EffectFailed(String),
    /// Invalid input parameter.
    InvalidInput(String),
    /// Kernel transition error.
    KernelError(String),
}

impl RuntimeError {
    /// Create an unauthorized error with a message.
    #[inline]
    #[must_use]
    pub fn unauthorized(msg: impl Into<String>) -> Self {
        Self::Unauthorized(msg.into())
    }

    /// Create an insufficient balance error.
    #[inline]
    #[must_use]
    pub const fn insufficient_balance(available: u128, required: u128) -> Self {
        Self::InsufficientBalance { available, required }
    }

    /// Create an invalid state error.
    #[inline]
    #[must_use]
    pub fn invalid_state(msg: impl Into<String>) -> Self {
        Self::InvalidState(msg.into())
    }

    /// Create a storage error.
    #[inline]
    #[must_use]
    pub fn storage_error(msg: impl Into<String>) -> Self {
        Self::StorageError(msg.into())
    }

    /// Create an effect failed error.
    #[inline]
    #[must_use]
    pub fn effect_failed(msg: impl Into<String>) -> Self {
        Self::EffectFailed(msg.into())
    }

    /// Create an invalid input error.
    #[inline]
    #[must_use]
    pub fn invalid_input(msg: impl Into<String>) -> Self {
        Self::InvalidInput(msg.into())
    }

    /// Create a kernel error.
    #[inline]
    #[must_use]
    pub fn kernel_error(msg: impl Into<String>) -> Self {
        Self::KernelError(msg.into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_constructors() {
        let err = RuntimeError::unauthorized("not allowed");
        assert!(matches!(err, RuntimeError::Unauthorized(_)));

        let err = RuntimeError::insufficient_balance(100, 200);
        assert!(matches!(
            err,
            RuntimeError::InsufficientBalance {
                available: 100,
                required: 200
            }
        ));

        let err = RuntimeError::invalid_state("wrong state");
        assert!(matches!(err, RuntimeError::InvalidState(_)));

        let err = RuntimeError::storage_error("storage failed");
        assert!(matches!(err, RuntimeError::StorageError(_)));

        let err = RuntimeError::effect_failed("effect failed");
        assert!(matches!(err, RuntimeError::EffectFailed(_)));

        let err = RuntimeError::invalid_input("bad input");
        assert!(matches!(err, RuntimeError::InvalidInput(_)));

        let err = RuntimeError::kernel_error("kernel failed");
        assert!(matches!(err, RuntimeError::KernelError(_)));
    }
}
