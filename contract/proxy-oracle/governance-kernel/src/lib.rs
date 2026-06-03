#![no_std]

extern crate alloc;
#[cfg(feature = "std")]
extern crate std;

#[cfg(any(feature = "borsh", feature = "schemars"))]
use alloc::format;
#[cfg(feature = "borsh")]
use alloc::string::ToString;
use alloc::vec::Vec;
use core::fmt;

use templar_primitives::Nanoseconds;

macro_rules! serialize {
    ($i:item) => {
        #[cfg_attr(
            feature = "borsh",
            derive(
                ::borsh::BorshSerialize,
                ::borsh::BorshDeserialize,
                ::borsh::BorshSchema
            )
        )]
        #[cfg_attr(feature = "schemars", derive(::schemars::JsonSchema))]
        #[cfg_attr(feature = "serde", derive(::serde::Serialize, ::serde::Deserialize))]
        $i
    };
}

serialize! {
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Proposal<Operation, AccountId> {
    pub operation: Operation,
    pub created_at: Nanoseconds,
    pub ttl: Nanoseconds,
    pub created_by: AccountId,
}
}

impl<Operation, AccountId> Proposal<Operation, AccountId> {
    #[must_use]
    pub fn can_execute(&self, now: Nanoseconds) -> bool {
        now.saturating_sub(self.created_at) >= self.ttl
    }
}

serialize! {
/// The governance proposal ledger: a small, owned index over the pending
/// proposal queue. It tracks only `next_id`, the set of pending ids, the
/// per-operation TTL table, and the pending cap — it does **not** own the
/// proposal bodies. Each runtime stores the bodies at whatever granularity is
/// efficient for it (one storage key per proposal) and hands the relevant body
/// back to the kernel when it needs to validate a transition. This keeps the
/// kernel from dictating a storage layout while still owning every invariant of
/// the proposal lifecycle.
///
/// Role/authorization state lives entirely in the runtime layer (each runtime
/// uses its own audited RBAC primitive); the kernel performs no role checks.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Governance<TtlConfig> {
    pub next_id: u64,
    pub active_ids: Vec<u64>,
    pub ttls: TtlConfig,
    pub max_pending_proposals: u32,
}
}

pub trait TtlConfig<Kind: Copy> {
    fn get(&self, kind: Kind) -> Nanoseconds;
    fn set(&mut self, kind: Kind, ttl: Nanoseconds);
}

/// The runtime-defined policy for an operation type. The kernel uses this to
/// compute the minimum timelock for an operation and to run the operation's
/// own create/execute validation. Role mapping (`required_role`) stays an
/// inherent concern of the runtime, since authorization is enforced there.
pub trait OperationPolicy<Ttls> {
    type OnCreateError;
    type OnExecuteError;

    fn minimum_ttl(&self, ttls: &Ttls) -> Nanoseconds;

    fn validate_on_create(&self) -> Result<(), Self::OnCreateError>;
    fn validate_on_execute(&self) -> Result<(), Self::OnExecuteError>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IdOutOfOrderError {
    pub expected: u64,
    pub actual: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IdOutOfBoundsError {
    pub exclusive_maximum: u64,
    pub actual: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProposalDoesNotExistError {
    pub id: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TtlNotElapsedError {
    pub id: u64,
    pub now: Nanoseconds,
    pub created_at: Nanoseconds,
    pub ttl: Nanoseconds,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CreateError<E> {
    IdOutOfOrder(IdOutOfOrderError),
    IdOverflow,
    TooManyPendingProposals,
    Validation(E),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CancelError {
    IdOutOfBounds(IdOutOfBoundsError),
    ProposalDoesNotExist(ProposalDoesNotExistError),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExecuteError<E> {
    IdOutOfBounds(IdOutOfBoundsError),
    ProposalDoesNotExist(ProposalDoesNotExistError),
    TtlNotElapsed(TtlNotElapsedError),
    Validation(E),
}

impl fmt::Display for IdOutOfOrderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "ID is out-of-order: expected {}, got {}",
            self.expected, self.actual
        )
    }
}

impl fmt::Display for IdOutOfBoundsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "ID is out-of-bounds: exclusive maximum {}, got {}",
            self.exclusive_maximum, self.actual
        )
    }
}

impl fmt::Display for ProposalDoesNotExistError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "The proposal does not exist because it has already been cancelled or executed: {}",
            self.id
        )
    }
}

impl fmt::Display for TtlNotElapsedError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "TTL not yet elapsed for proposal {}: current timestamp {} < created at {} + TTL {}",
            self.id,
            self.now.as_ns(),
            self.created_at.as_ns(),
            self.ttl.as_ns()
        )
    }
}

impl<E: fmt::Display> fmt::Display for CreateError<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::IdOutOfOrder(error) => error.fmt(f),
            Self::IdOverflow => f.write_str("proposal ID overflow"),
            Self::TooManyPendingProposals => f.write_str("too many pending proposals"),
            Self::Validation(error) => write!(f, "Validation error: {error}"),
        }
    }
}

impl fmt::Display for CancelError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::IdOutOfBounds(error) => error.fmt(f),
            Self::ProposalDoesNotExist(error) => error.fmt(f),
        }
    }
}

impl<E: fmt::Display> fmt::Display for ExecuteError<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::IdOutOfBounds(error) => error.fmt(f),
            Self::ProposalDoesNotExist(error) => error.fmt(f),
            Self::TtlNotElapsed(error) => error.fmt(f),
            Self::Validation(error) => write!(f, "Validation error: {error}"),
        }
    }
}

impl<E> From<IdOutOfOrderError> for CreateError<E> {
    fn from(value: IdOutOfOrderError) -> Self {
        Self::IdOutOfOrder(value)
    }
}

impl From<IdOutOfBoundsError> for CancelError {
    fn from(value: IdOutOfBoundsError) -> Self {
        Self::IdOutOfBounds(value)
    }
}

impl From<ProposalDoesNotExistError> for CancelError {
    fn from(value: ProposalDoesNotExistError) -> Self {
        Self::ProposalDoesNotExist(value)
    }
}

impl<E> From<IdOutOfBoundsError> for ExecuteError<E> {
    fn from(value: IdOutOfBoundsError) -> Self {
        Self::IdOutOfBounds(value)
    }
}

impl<E> From<ProposalDoesNotExistError> for ExecuteError<E> {
    fn from(value: ProposalDoesNotExistError) -> Self {
        Self::ProposalDoesNotExist(value)
    }
}

impl<E> From<TtlNotElapsedError> for ExecuteError<E> {
    fn from(value: TtlNotElapsedError) -> Self {
        Self::TtlNotElapsed(value)
    }
}

impl<Ttls> Governance<Ttls> {
    #[must_use]
    pub fn new(ttls: Ttls, max_pending_proposals: u32) -> Self {
        Self {
            next_id: 0,
            active_ids: Vec::new(),
            ttls,
            max_pending_proposals,
        }
    }

    /// The full slice of currently-pending proposal ids in insertion order.
    /// Callers derive count (`.len()`), iteration, and membership
    /// (`.contains(&id)`) directly from this — no separate accessors needed.
    #[must_use]
    pub fn active_ids(&self) -> &[u64] {
        &self.active_ids
    }

    /// Validates and reserves a new proposal id, returning the proposal body
    /// for the runtime to persist. The kernel records the id as pending and
    /// advances `next_id`; it never stores the body itself.
    ///
    /// # Errors
    ///
    /// If the id is out-of-order, the pending cap is reached, the id counter
    /// overflows, or the operation fails its create-time validation.
    pub fn create<Operation, AccountId>(
        &mut self,
        id: u64,
        operation: Operation,
        now: Nanoseconds,
        created_by: AccountId,
        ttl: Nanoseconds,
    ) -> Result<Proposal<Operation, AccountId>, CreateError<Operation::OnCreateError>>
    where
        Operation: OperationPolicy<Ttls>,
    {
        if id != self.next_id {
            return Err(IdOutOfOrderError {
                expected: self.next_id,
                actual: id,
            }
            .into());
        }
        if u32::try_from(self.active_ids.len()).unwrap_or(u32::MAX) >= self.max_pending_proposals {
            return Err(CreateError::TooManyPendingProposals);
        }
        operation
            .validate_on_create()
            .map_err(CreateError::Validation)?;
        self.next_id = self.next_id.checked_add(1).ok_or(CreateError::IdOverflow)?;
        self.active_ids.push(id);
        Ok(Proposal {
            operation,
            created_at: now,
            ttl,
            created_by,
        })
    }

    /// Removes a pending proposal id without executing it. The runtime is
    /// responsible for deleting the corresponding body.
    ///
    /// # Errors
    ///
    /// If the id was never issued or is no longer pending.
    pub fn cancel(&mut self, id: u64) -> Result<(), CancelError> {
        let index = self.pending_index(id).map_err(CancelError::from)?;
        self.active_ids.remove(index);
        Ok(())
    }

    /// Validates that `proposal` (loaded by the runtime for `id`) is mature and
    /// passes execute-time validation, then removes the id from the pending
    /// set. The runtime deletes the body and performs the side effects.
    ///
    /// # Errors
    ///
    /// If the id was never issued or is no longer pending, the proposal's TTL
    /// has not elapsed, or the operation fails its execute-time validation.
    pub fn execute<Operation, AccountId>(
        &mut self,
        id: u64,
        proposal: &Proposal<Operation, AccountId>,
        now: Nanoseconds,
    ) -> Result<(), ExecuteError<Operation::OnExecuteError>>
    where
        Operation: OperationPolicy<Ttls>,
    {
        let index = self.pending_index(id).map_err(ExecuteError::from)?;
        proposal
            .operation
            .validate_on_execute()
            .map_err(ExecuteError::Validation)?;
        if !proposal.can_execute(now) {
            return Err(TtlNotElapsedError {
                id,
                now,
                created_at: proposal.created_at,
                ttl: proposal.ttl,
            }
            .into());
        }
        self.active_ids.remove(index);
        Ok(())
    }

    #[must_use]
    pub fn effective_ttl<Operation>(
        &self,
        operation: &Operation,
        requested_ttl: Nanoseconds,
    ) -> Nanoseconds
    where
        Operation: OperationPolicy<Ttls>,
    {
        operation.minimum_ttl(&self.ttls).max(requested_ttl)
    }

    /// Resolves the position of a pending id, distinguishing "never issued"
    /// (out-of-bounds) from "already cancelled or executed" (does-not-exist).
    fn pending_index(&self, id: u64) -> Result<usize, PendingLookupError> {
        if id >= self.next_id {
            return Err(IdOutOfBoundsError {
                exclusive_maximum: self.next_id,
                actual: id,
            }
            .into());
        }
        self.active_ids
            .iter()
            .position(|candidate| *candidate == id)
            .ok_or(ProposalDoesNotExistError { id }.into())
    }
}

/// Internal: the two ways a pending-id lookup can fail, shared by cancel and
/// execute (which widen it into their own error enums).
enum PendingLookupError {
    IdOutOfBounds(IdOutOfBoundsError),
    ProposalDoesNotExist(ProposalDoesNotExistError),
}

impl From<IdOutOfBoundsError> for PendingLookupError {
    fn from(value: IdOutOfBoundsError) -> Self {
        Self::IdOutOfBounds(value)
    }
}

impl From<ProposalDoesNotExistError> for PendingLookupError {
    fn from(value: ProposalDoesNotExistError) -> Self {
        Self::ProposalDoesNotExist(value)
    }
}

impl From<PendingLookupError> for CancelError {
    fn from(value: PendingLookupError) -> Self {
        match value {
            PendingLookupError::IdOutOfBounds(error) => Self::IdOutOfBounds(error),
            PendingLookupError::ProposalDoesNotExist(error) => Self::ProposalDoesNotExist(error),
        }
    }
}

impl<E> From<PendingLookupError> for ExecuteError<E> {
    fn from(value: PendingLookupError) -> Self {
        match value {
            PendingLookupError::IdOutOfBounds(error) => Self::IdOutOfBounds(error),
            PendingLookupError::ProposalDoesNotExist(error) => Self::ProposalDoesNotExist(error),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum Kind {
        Slow,
        Fast,
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    struct Ttls {
        slow: Nanoseconds,
        fast: Nanoseconds,
    }

    impl TtlConfig<Kind> for Ttls {
        fn get(&self, kind: Kind) -> Nanoseconds {
            match kind {
                Kind::Slow => self.slow,
                Kind::Fast => self.fast,
            }
        }

        fn set(&mut self, kind: Kind, ttl: Nanoseconds) {
            match kind {
                Kind::Slow => self.slow = ttl,
                Kind::Fast => self.fast = ttl,
            }
        }
    }

    #[derive(Clone, Debug, PartialEq, Eq)]
    struct Op(&'static str, Kind);

    impl OperationPolicy<Ttls> for Op {
        type OnCreateError = ();
        type OnExecuteError = ();

        fn minimum_ttl(&self, ttls: &Ttls) -> Nanoseconds {
            ttls.get(self.1)
        }

        fn validate_on_create(&self) -> Result<(), Self::OnCreateError> {
            Ok(())
        }

        fn validate_on_execute(&self) -> Result<(), Self::OnExecuteError> {
            Ok(())
        }
    }

    fn governance() -> Governance<Ttls> {
        Governance::new(
            Ttls {
                slow: Nanoseconds::from_secs(10),
                fast: Nanoseconds::zero(),
            },
            64,
        )
    }

    #[test]
    fn creates_in_order_and_executes_out_of_order() {
        let mut governance = governance();
        let now = Nanoseconds::from_secs(1);

        let slow = governance
            .create(0, Op("slow", Kind::Slow), now, "alice", Nanoseconds::zero())
            .unwrap();
        let fast = governance
            .create(1, Op("fast", Kind::Fast), now, "alice", Nanoseconds::zero())
            .unwrap();
        assert_eq!(governance.active_ids().len(), 2);

        governance.execute(1, &fast, now).unwrap();
        assert_eq!(governance.active_ids().len(), 1);
        assert!(governance.active_ids().contains(&0));
        assert!(!governance.active_ids().contains(&1));

        governance.execute(0, &slow, now).unwrap();
        assert_eq!(governance.active_ids().len(), 0);
    }

    #[test]
    fn rejects_out_of_order_creation() {
        let mut governance = governance();

        assert_eq!(
            governance
                .create(
                    1,
                    Op("fast", Kind::Fast),
                    Nanoseconds::zero(),
                    "alice",
                    Nanoseconds::zero(),
                )
                .unwrap_err(),
            CreateError::IdOutOfOrder(IdOutOfOrderError {
                expected: 0,
                actual: 1,
            })
        );
    }

    #[test]
    fn execute_enforces_ttl() {
        let mut governance = governance();
        let created_at = Nanoseconds::from_secs(1);
        let ttl = Nanoseconds::from_secs(10);

        let proposal = governance
            .create(0, Op("slow", Kind::Slow), created_at, "alice", ttl)
            .unwrap();

        assert!(matches!(
            governance.execute(0, &proposal, created_at).unwrap_err(),
            ExecuteError::TtlNotElapsed(_)
        ));
        assert!(governance.active_ids().contains(&0));

        governance
            .execute(0, &proposal, Nanoseconds::from_secs(11))
            .unwrap();
        assert!(!governance.active_ids().contains(&0));
    }

    #[test]
    fn cancel_removes_pending_proposal() {
        let mut governance = governance();
        let now = Nanoseconds::from_secs(1);

        governance
            .create(0, Op("x", Kind::Fast), now, "alice", Nanoseconds::zero())
            .unwrap();
        governance.cancel(0).unwrap();
        assert!(!governance.active_ids().contains(&0));

        assert!(matches!(
            governance.cancel(0).unwrap_err(),
            CancelError::ProposalDoesNotExist(_)
        ));
        assert!(matches!(
            governance.cancel(5).unwrap_err(),
            CancelError::IdOutOfBounds(_)
        ));
    }

    #[test]
    fn enforces_max_pending_proposals() {
        let mut governance = Governance::new(
            Ttls {
                slow: Nanoseconds::zero(),
                fast: Nanoseconds::zero(),
            },
            2,
        );
        let now = Nanoseconds::zero();

        governance
            .create(0, Op("a", Kind::Fast), now, "alice", Nanoseconds::zero())
            .unwrap();
        governance
            .create(1, Op("b", Kind::Fast), now, "alice", Nanoseconds::zero())
            .unwrap();
        assert_eq!(
            governance
                .create(2, Op("c", Kind::Fast), now, "alice", Nanoseconds::zero())
                .unwrap_err(),
            CreateError::TooManyPendingProposals
        );
    }
}
