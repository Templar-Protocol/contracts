use near_api::types::{Reference, TxExecutionStatus};

/// A coherent transaction-wait and state-query policy.
///
/// Every mode waits for a complete application execution outcome. We
/// intentionally do not expose `None`, `Included`, or `IncludedFinal`, which can
/// return a pending transaction result that the operation driver cannot persist
/// as a completed step.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum FinalityPolicy {
    /// Wait until the transaction is included in a finalized block and all
    /// non-refund receipts execute. Receipt blocks may still be unfinalized, so
    /// reads use optimistic state. This is the production default.
    #[default]
    Executed,
    /// Wait until every receipt, including gas refunds, executes in finalized
    /// blocks and read finalized state.
    Final,
    /// Wait until all non-refund receipts execute optimistically and read the
    /// corresponding optimistic state.
    ExecutedOptimistic,
}

impl FinalityPolicy {
    #[must_use]
    pub const fn transaction_status(self) -> TxExecutionStatus {
        match self {
            Self::Executed => TxExecutionStatus::Executed,
            Self::Final => TxExecutionStatus::Final,
            Self::ExecutedOptimistic => TxExecutionStatus::ExecutedOptimistic,
        }
    }

    #[must_use]
    pub const fn query_reference(self) -> Reference {
        match self {
            Self::Executed | Self::ExecutedOptimistic => Reference::Optimistic,
            Self::Final => Reference::Final,
        }
    }
}

#[cfg(test)]
mod tests {
    use near_api::types::{Reference, TxExecutionStatus};

    use super::FinalityPolicy;

    #[test]
    fn default_policy_finalizes_inclusion_and_reads_executed_receipt_state() {
        let policy = FinalityPolicy::default();

        assert_eq!(policy, FinalityPolicy::Executed);
        assert_eq!(policy.transaction_status(), TxExecutionStatus::Executed);
        assert!(matches!(policy.query_reference(), Reference::Optimistic));
    }

    #[test]
    fn final_policy_waits_for_refunds_and_reads_finalized_state() {
        let policy = FinalityPolicy::Final;

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
