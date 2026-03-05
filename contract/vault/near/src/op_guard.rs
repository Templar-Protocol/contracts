use std::ops::{Deref, DerefMut};

use templar_common::{
    guard::{Guard, GuardSpec},
    panic_with_message,
    vault::{
        AllocatingState, Error, IdleState, OpState, PayoutState, RefreshingState, WithdrawingState,
    },
};

use crate::Contract;

macro_rules! impl_op_guard_spec {
    ($spec:ident, $state:ty, $variant:ident, $error:expr) => {
        impl GuardSpec<Contract> for $spec {
            type State = $state;
            type Error = Error;
            type Idle = IdleSpec;

            fn validate(
                target: &Contract,
                op_id: Option<u64>,
            ) -> Result<&Self::State, Self::Error> {
                match &target.op_state {
                    OpState::$variant(state) if op_id.map_or(true, |id| state.op_id == id) => {
                        Ok(state)
                    }
                    _ => Err($error),
                }
            }

            fn set_state(target: &mut Contract, state: Self::State) {
                target.op_state = OpState::$variant(state);
            }

            fn into_idle(target: &mut Contract) {
                target.op_state = OpState::Idle;
            }
        }
    };
}

pub(crate) struct IdleSpec;
pub(crate) struct AllocatingSpec;
pub(crate) struct WithdrawingSpec;
pub(crate) struct RefreshingSpec;
pub(crate) struct PayoutSpec;

pub(crate) struct OpGuard<'a, S: GuardSpec<Contract>>(Guard<'a, Contract, S>);

pub(crate) type IdleGuard<'a> = OpGuard<'a, IdleSpec>;
pub(crate) type PayoutGuard<'a> = OpGuard<'a, PayoutSpec>;

impl<'a, S: GuardSpec<Contract>> OpGuard<'a, S> {
    pub fn expect(contract: &'a mut Contract, op_id: Option<u64>) -> Result<Self, S::Error> {
        Guard::expect(contract, op_id).map(Self)
    }

    pub fn state(&self) -> &S::State {
        self.0.state()
    }

    pub fn into_idle(self) -> OpGuard<'a, S::Idle> {
        OpGuard(self.0.into_idle())
    }

    pub fn into_inner(self) -> &'a mut Contract {
        self.0.into_inner()
    }
}

impl<'a, S: GuardSpec<Contract>> Deref for OpGuard<'a, S> {
    type Target = Contract;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<'a, S: GuardSpec<Contract>> DerefMut for OpGuard<'a, S> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl GuardSpec<Contract> for IdleSpec {
    type State = IdleState;
    type Error = Error;
    type Idle = IdleSpec;

    fn validate(target: &Contract, _op_id: Option<u64>) -> Result<&Self::State, Self::Error> {
        match &target.op_state {
            OpState::Idle => Ok(&IdleState),
            op_state => panic_with_message(&format!(
                "Invariant: Only one op in flight; current op_state = {:?}",
                op_state
            )),
        }
    }

    fn set_state(target: &mut Contract, _state: Self::State) {
        target.op_state = OpState::Idle;
    }

    fn into_idle(target: &mut Contract) {
        target.op_state = OpState::Idle;
    }
}

impl_op_guard_spec!(
    AllocatingSpec,
    AllocatingState,
    Allocating,
    Error::NotAllocating
);
impl_op_guard_spec!(
    WithdrawingSpec,
    WithdrawingState,
    Withdrawing,
    Error::NotWithdrawing
);
impl_op_guard_spec!(
    RefreshingSpec,
    RefreshingState,
    Refreshing,
    Error::NotRefreshing
);
impl_op_guard_spec!(PayoutSpec, PayoutState, Payout, Error::NotPayout);

impl<'a> OpGuard<'a, IdleSpec> {
    pub fn new(contract: &'a mut Contract) -> Self {
        Self::expect(contract, None)
            .unwrap_or_else(|e| panic_with_message(&format!("idle guard: {e}")))
    }
}

impl<'a> OpGuard<'a, WithdrawingSpec> {
    pub fn into_payout(self, state: PayoutState) -> PayoutGuard<'a> {
        let op_id = state.op_id;
        let contract = self.into_inner();
        PayoutSpec::set_state(contract, state);
        PayoutGuard::expect(contract, Some(op_id))
            .unwrap_or_else(|e| panic_with_message(&format!("payout guard: {e}")))
    }
}
