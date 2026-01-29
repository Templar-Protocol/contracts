use crate::restrictions::Restrictions;
use crate::transitions::TransitionError;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KernelError {
    /// Action not allowed in the current state.
    InvalidState(&'static str),
    /// Operation ID mismatch.
    OpIdMismatch { expected: u64, actual: u64 },
    /// Slippage guard failed.
    Slippage { min: u128, actual: u128 },
    /// Withdrawal below configured minimum.
    MinWithdrawal { amount: u128, min: u128 },
    /// Queue at capacity.
    QueueFull,
    /// No pending withdrawals available.
    EmptyQueue,
    /// Withdrawal request is still in cooldown.
    Cooldown { requested_at: u64, now: u64, cooldown_ns: u64 },
    /// Transition error from op-state machine.
    Transition(TransitionError),
    /// Action not implemented yet.
    NotImplemented,
    /// Action blocked by restrictions.
    Restricted(Restrictions),
}
