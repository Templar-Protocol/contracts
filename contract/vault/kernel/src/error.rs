//! Kernel error types.

use crate::restrictions::Restrictions;
use crate::transitions::TransitionError;
use derive_more::Display;

/// Errors that can occur when applying kernel actions.
#[derive(Clone, Debug, PartialEq, Eq, Display)]
pub enum KernelError {
    /// Action not allowed in the current state.
    #[display("{_0}")]
    InvalidState(&'static str),

    /// Operation ID mismatch between expected and actual.
    #[display("op_id mismatch: expected {expected}, got {actual}")]
    OpIdMismatch { expected: u64, actual: u64 },

    /// Slippage guard failed - output below minimum.
    #[display("slippage: minimum {min}, actual {actual}")]
    Slippage { min: u128, actual: u128 },

    /// Withdrawal amount below configured minimum.
    #[display("withdrawal {amount} below minimum {min}")]
    MinWithdrawal { amount: u128, min: u128 },

    /// Withdrawal queue is at capacity.
    #[display("withdrawal queue full")]
    QueueFull,

    /// No pending withdrawals available to execute.
    #[display("withdrawal queue empty")]
    EmptyQueue,

    /// Withdrawal request is still in cooldown period.
    #[display("cooldown: requested at {requested_at}, now {now}, cooldown {cooldown_ns}ns")]
    Cooldown {
        requested_at: u64,
        now: u64,
        cooldown_ns: u64,
    },

    /// Transition error from the op-state machine.
    #[display("transition error: {_0}")]
    Transition(TransitionError),

    /// Action not implemented yet.
    #[display("not implemented")]
    NotImplemented,

    /// Action blocked by access restrictions.
    #[display("restricted: {_0:?}")]
    Restricted(Restrictions),

    /// Invalid configuration value.
    #[display("invalid config: {_0}")]
    InvalidConfig(&'static str),
}
