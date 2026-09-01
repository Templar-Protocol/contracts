use near_api::types::{Reference, TxExecutionStatus};

/// A coherent transaction-wait and state-query policy for *submitting* a
/// transaction and reading the state it produced.
///
/// Every mode waits for a complete application execution outcome. We
/// intentionally do not expose `None`, `Included`, or `IncludedFinal`, which can
/// return a pending transaction result that the operation driver cannot persist
/// as a completed step.
///
/// Reconciliation does not use it to wait: it asks what the chain has now, and
/// applies this as the bar an answer must meet to count as an outcome.
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

    /// Whether `progress` meets this policy's bar. Spelled out rather than
    /// compared: the levels are not a ladder — `ExecutedOptimistic` has execution
    /// without finality and `IncludedFinal` finality without execution — so
    /// `TxExecutionStatus`'s derived `Ord` does not mean what it appears to.
    #[must_use]
    pub const fn is_satisfied_by(self, progress: TxExecutionStatus) -> bool {
        match self {
            Self::ExecutedOptimistic => matches!(
                progress,
                TxExecutionStatus::ExecutedOptimistic
                    | TxExecutionStatus::Executed
                    | TxExecutionStatus::Final
            ),
            Self::Executed => matches!(
                progress,
                TxExecutionStatus::Executed | TxExecutionStatus::Final
            ),
            Self::Final => matches!(progress, TxExecutionStatus::Final),
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

    use rstest::rstest;

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

    /// Every level against every policy, since the levels are not a ladder.
    #[rstest]
    #[case(
        FinalityPolicy::ExecutedOptimistic,
        &[
            TxExecutionStatus::ExecutedOptimistic,
            TxExecutionStatus::Executed,
            TxExecutionStatus::Final,
        ]
    )]
    #[case(
        FinalityPolicy::Executed,
        &[TxExecutionStatus::Executed, TxExecutionStatus::Final]
    )]
    #[case(FinalityPolicy::Final, &[TxExecutionStatus::Final])]
    fn policy_is_satisfied_only_by_a_level_that_meets_it(
        #[case] policy: FinalityPolicy,
        #[case] accepted: &[TxExecutionStatus],
    ) {
        for level in [
            TxExecutionStatus::None,
            TxExecutionStatus::Included,
            TxExecutionStatus::ExecutedOptimistic,
            TxExecutionStatus::IncludedFinal,
            TxExecutionStatus::Executed,
            TxExecutionStatus::Final,
        ] {
            assert_eq!(
                policy.is_satisfied_by(level),
                accepted.contains(&level),
                "{policy:?} against {level:?}"
            );
        }
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
