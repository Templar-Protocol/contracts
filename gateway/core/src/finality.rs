use near_api::types::{Reference, TxExecutionStatus};

/// A coherent transaction-wait and state-query policy for *submitting* a
/// transaction and reading the state it produced.
///
/// Every mode waits for a complete application execution outcome. We
/// intentionally do not expose `None`, `Included`, or `IncludedFinal`, which can
/// return a pending transaction result that the operation driver cannot persist
/// as a completed step.
///
/// This does not govern reconciliation, which looks up an already-submitted
/// transaction and has a well-defined answer for "not finished yet" — leave the
/// step submitted. It asks without waiting; see `RECONCILIATION_WAIT_UNTIL`.
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

    /// Whether `progress` meets this policy's bar.
    ///
    /// Spelled out rather than compared: nearcore's levels are not a ladder —
    /// `ExecutedOptimistic` has execution without finality and `IncludedFinal`
    /// finality without execution — so `TxExecutionStatus`'s derived `Ord`,
    /// which follows declaration order, does not mean what it appears to.
    #[must_use]
    pub fn is_satisfied_by(self, progress: &TxExecutionStatus) -> bool {
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

    /// All 18 pairings, because the levels are not a ladder. `ExecutedOptimistic`
    /// has execution without finality and `IncludedFinal` finality without
    /// execution — neither satisfies the other, and `TxExecutionStatus`'s derived
    /// `Ord` (declaration order) would get both wrong.
    #[rstest]
    #[case::optimistic_accepts_itself(
        FinalityPolicy::ExecutedOptimistic,
        TxExecutionStatus::ExecutedOptimistic,
        true
    )]
    #[case::optimistic_accepts_higher(
        FinalityPolicy::ExecutedOptimistic,
        TxExecutionStatus::Final,
        true
    )]
    #[case::optimistic_rejects_included_final(
        FinalityPolicy::ExecutedOptimistic,
        TxExecutionStatus::IncludedFinal,
        false
    )]
    #[case::optimistic_rejects_included(
        FinalityPolicy::ExecutedOptimistic,
        TxExecutionStatus::Included,
        false
    )]
    #[case::optimistic_rejects_none(
        FinalityPolicy::ExecutedOptimistic,
        TxExecutionStatus::None,
        false
    )]
    #[case::executed_accepts_itself(FinalityPolicy::Executed, TxExecutionStatus::Executed, true)]
    #[case::executed_accepts_final(FinalityPolicy::Executed, TxExecutionStatus::Final, true)]
    #[case::executed_rejects_optimistic(
        FinalityPolicy::Executed,
        TxExecutionStatus::ExecutedOptimistic,
        false
    )]
    #[case::executed_rejects_included_final(
        FinalityPolicy::Executed,
        TxExecutionStatus::IncludedFinal,
        false
    )]
    #[case::final_accepts_only_final(FinalityPolicy::Final, TxExecutionStatus::Final, true)]
    #[case::final_rejects_executed(FinalityPolicy::Final, TxExecutionStatus::Executed, false)]
    #[case::final_rejects_optimistic(
        FinalityPolicy::Final,
        TxExecutionStatus::ExecutedOptimistic,
        false
    )]
    #[case::optimistic_accepts_executed(
        FinalityPolicy::ExecutedOptimistic,
        TxExecutionStatus::Executed,
        true
    )]
    #[case::executed_rejects_included(FinalityPolicy::Executed, TxExecutionStatus::Included, false)]
    #[case::executed_rejects_none(FinalityPolicy::Executed, TxExecutionStatus::None, false)]
    #[case::final_rejects_included_final(
        FinalityPolicy::Final,
        TxExecutionStatus::IncludedFinal,
        false
    )]
    #[case::final_rejects_included(FinalityPolicy::Final, TxExecutionStatus::Included, false)]
    #[case::final_rejects_none(FinalityPolicy::Final, TxExecutionStatus::None, false)]
    fn policy_is_satisfied_only_by_a_level_that_meets_it(
        #[case] policy: FinalityPolicy,
        #[case] progress: TxExecutionStatus,
        #[case] expected: bool,
    ) {
        assert_eq!(policy.is_satisfied_by(&progress), expected);
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
