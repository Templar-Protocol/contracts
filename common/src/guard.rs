use std::marker::PhantomData;
use std::ops::{Deref, DerefMut};

pub trait GuardSpec<T> {
    type State;
    type Error;
    type Idle: GuardSpec<T, State = Self::State, Error = Self::Error>;

    fn validate(target: &T, op_id: Option<u64>) -> Result<&Self::State, Self::Error>;
    fn set_state(target: &mut T, state: Self::State);
    fn into_idle(target: &mut T);
}

pub struct Guard<'a, T, S: GuardSpec<T>> {
    target: &'a mut T,
    _marker: PhantomData<S>,
}

impl<'a, T, S: GuardSpec<T>> Guard<'a, T, S> {
    pub fn expect(target: &'a mut T, op_id: Option<u64>) -> Result<Self, S::Error> {
        let _ = S::validate(target, op_id)?;
        Ok(Self {
            target,
            _marker: PhantomData,
        })
    }

    pub fn state(&self) -> &S::State {
        S::validate(self.target, None).expect("validated state")
    }

    pub fn replace_state(self, state: S::State) -> Self {
        S::set_state(self.target, state);
        Self {
            target: self.target,
            _marker: PhantomData,
        }
    }

    pub fn into_idle(self) -> Guard<'a, T, S::Idle> {
        S::into_idle(self.target);
        Guard {
            target: self.target,
            _marker: PhantomData,
        }
    }

    pub fn contract(&mut self) -> &mut T {
        self.target
    }
}

impl<'a, T, S: GuardSpec<T>> Deref for Guard<'a, T, S> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        self.target
    }
}

impl<'a, T, S: GuardSpec<T>> DerefMut for Guard<'a, T, S> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.target
    }
}
