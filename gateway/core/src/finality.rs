use near_api::types::{Reference, TxExecutionStatus};

/// A coherent transaction-wait and state-query policy.
///
/// Both supported modes wait for a complete execution outcome. We intentionally
/// do not expose `None` or `Included`, which can return a pending transaction
/// result that the operation driver cannot persist as a completed step.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum FinalityPolicy {
    /// Wait for final execution and read finalized state. This is the production
    /// default.
    #[default]
    Final,
    /// Wait until all transaction receipts execute optimistically and read the
    /// corresponding optimistic state.
    ExecutedOptimistic,
}

impl FinalityPolicy {
    #[must_use]
    pub const fn transaction_status(self) -> TxExecutionStatus {
        match self {
            Self::Final => TxExecutionStatus::Final,
            Self::ExecutedOptimistic => TxExecutionStatus::ExecutedOptimistic,
        }
    }

    #[must_use]
    pub const fn query_reference(self) -> Reference {
        match self {
            Self::Final => Reference::Final,
            Self::ExecutedOptimistic => Reference::Optimistic,
        }
    }
}

#[cfg(test)]
mod tests {
    use near_api::types::{Reference, TxExecutionStatus};

    use super::FinalityPolicy;

    #[test]
    fn default_policy_is_final() {
        let policy = FinalityPolicy::default();

        assert_eq!(policy, FinalityPolicy::Final);
        assert_eq!(policy.transaction_status(), TxExecutionStatus::Final);
        assert!(matches!(policy.query_reference(), Reference::Final));
    }

    #[test]
    fn optimistic_policy_keeps_transactions_and_queries_aligned() {
        let policy = FinalityPolicy::ExecutedOptimistic;

        assert_eq!(
            policy.transaction_status(),
            TxExecutionStatus::ExecutedOptimistic
        );
        assert!(matches!(policy.query_reference(), Reference::Optimistic));
    }
}
