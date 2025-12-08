use crate::{AllocatingState, Contract, Error, OpState, PayoutState, WithdrawingState};
use std::marker::PhantomData;
use std::ops::{Deref, DerefMut};
use templar_common::panic_with_message;

pub trait GuardSpec {
    type State;
    fn validate(op_state: &OpState, op_id: Option<u64>) -> Result<&Self::State, Error>;
    fn set_state(contract: &mut Contract, state: Self::State);
    fn into_idle(contract: &mut Contract);
}

pub struct Guard<'a, S: GuardSpec> {
    contract: &'a mut Contract,
    _marker: PhantomData<S>,
}

impl<'a, S: GuardSpec> Deref for Guard<'a, S> {
    type Target = Contract;

    fn deref(&self) -> &Self::Target {
        self.contract
    }
}

impl<'a, S: GuardSpec> DerefMut for Guard<'a, S> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.contract
    }
}

impl<'a, S: GuardSpec> Guard<'a, S> {
    pub fn expect(contract: &'a mut Contract, op_id: Option<u64>) -> Result<Self, Error> {
        let _ = S::validate(&contract.op_state, op_id)?;
        Ok(Self {
            contract,
            _marker: PhantomData,
        })
    }

    pub fn state(&self) -> &S::State {
        // Safe because expect validated variant
        S::validate(&self.contract.op_state, None).expect("validated state")
    }

    pub fn replace_state(self, state: S::State) -> Self {
        S::set_state(self.contract, state);
        Self {
            contract: self.contract,
            _marker: PhantomData,
        }
    }

    pub fn into_idle(self) -> Guard<'a, IdleSpec> {
        S::into_idle(self.contract);
        Guard {
            contract: self.contract,
            _marker: PhantomData,
        }
    }

    pub fn contract(&mut self) -> &mut Contract {
        self.contract
    }
}

pub struct IdleSpec;
impl GuardSpec for IdleSpec {
    type State = ();

    fn validate(op_state: &OpState, _op_id: Option<u64>) -> Result<&Self::State, Error> {
        match op_state {
            OpState::Idle => Ok(&()),
            _ => panic_with_message(&format!(
                "Invariant: Only one op in flight; current op_state = {:?}",
                op_state
            )),
        }
    }

    fn set_state(contract: &mut Contract, _state: Self::State) {
        contract.op_state = OpState::Idle;
    }

    fn into_idle(contract: &mut Contract) {
        contract.op_state = OpState::Idle;
    }
}

pub struct AllocatingSpec;
impl GuardSpec for AllocatingSpec {
    type State = AllocatingState;

    fn validate(op_state: &OpState, op_id: Option<u64>) -> Result<&Self::State, Error> {
        match op_state {
            OpState::Allocating(state) if op_id.map_or(true, |id| state.op_id == id) => Ok(state),
            _ => Err(Error::NotAllocating),
        }
    }

    fn set_state(contract: &mut Contract, state: Self::State) {
        contract.op_state = OpState::Allocating(state);
    }

    fn into_idle(contract: &mut Contract) {
        contract.op_state = OpState::Idle;
    }
}

pub struct WithdrawingSpec;
impl GuardSpec for WithdrawingSpec {
    type State = WithdrawingState;

    fn validate(op_state: &OpState, op_id: Option<u64>) -> Result<&Self::State, Error> {
        match op_state {
            OpState::Withdrawing(state) if op_id.map_or(true, |id| state.op_id == id) => Ok(state),
            _ => Err(Error::NotWithdrawing),
        }
    }

    fn set_state(contract: &mut Contract, state: Self::State) {
        contract.op_state = OpState::Withdrawing(state);
    }

    fn into_idle(contract: &mut Contract) {
        contract.op_state = OpState::Idle;
    }
}

pub struct PayoutSpec;
impl GuardSpec for PayoutSpec {
    type State = PayoutState;

    fn validate(op_state: &OpState, op_id: Option<u64>) -> Result<&Self::State, Error> {
        match op_state {
            OpState::Payout(state) if op_id.map_or(true, |id| state.op_id == id) => Ok(state),
            _ => Err(Error::NotPayout),
        }
    }

    fn set_state(contract: &mut Contract, state: Self::State) {
        contract.op_state = OpState::Payout(state);
    }

    fn into_idle(contract: &mut Contract) {
        contract.op_state = OpState::Idle;
    }
}

pub type IdleGuard<'a> = Guard<'a, IdleSpec>;
pub type AllocatingGuard<'a> = Guard<'a, AllocatingSpec>;
pub type WithdrawingGuard<'a> = Guard<'a, WithdrawingSpec>;
pub type PayoutGuard<'a> = Guard<'a, PayoutSpec>;

impl<'a> IdleGuard<'a> {
    pub fn new(contract: &'a mut Contract) -> Self {
        Guard::expect(contract, None).expect("idle guard")
    }

    pub fn start_allocation(self, state: AllocatingState) -> AllocatingGuard<'a> {
        AllocatingSpec::set_state(self.contract, state);
        AllocatingGuard {
            contract: self.contract,
            _marker: PhantomData,
        }
    }

    pub fn start_withdrawal(self, state: WithdrawingState) -> WithdrawingGuard<'a> {
        WithdrawingSpec::set_state(self.contract, state);
        WithdrawingGuard {
            contract: self.contract,
            _marker: PhantomData,
        }
    }

    pub fn start_payout(self, state: PayoutState) -> PayoutGuard<'a> {
        PayoutSpec::set_state(self.contract, state);
        PayoutGuard {
            contract: self.contract,
            _marker: PhantomData,
        }
    }

    pub fn stay_idle(self) -> &'a mut Contract {
        self.contract
    }
}

impl<'a> WithdrawingGuard<'a> {
    pub fn into_payout(self, state: PayoutState) -> PayoutGuard<'a> {
        PayoutSpec::set_state(self.contract, state);
        PayoutGuard {
            contract: self.contract,
            _marker: PhantomData,
        }
    }
}
