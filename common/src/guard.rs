use std::marker::PhantomData;
use std::ops::{Deref, DerefMut};

pub trait GuardSpec<T> {
    type State;
    type Error;
    type Idle: GuardSpec<T, Error = Self::Error>;

    /// # Errors
    /// Returns `Self::Error` if the target is not in the expected state.
    fn validate(target: &T, op_id: Option<u64>) -> Result<&Self::State, Self::Error>;
    fn set_state(target: &mut T, state: Self::State);
    fn into_idle(target: &mut T);
}

pub struct Guard<'a, T, S: GuardSpec<T>> {
    target: &'a mut T,
    _marker: PhantomData<S>,
}

impl<'a, T, S: GuardSpec<T>> Guard<'a, T, S> {
    /// # Errors
    /// Returns `S::Error` if the target is not in the expected state.
    pub fn expect(target: &'a mut T, op_id: Option<u64>) -> Result<Self, S::Error> {
        let _ = S::validate(target, op_id)?;
        Ok(Self {
            target,
            _marker: PhantomData,
        })
    }

    pub fn state(&self) -> &S::State {
        S::validate(self.target, None)
            .unwrap_or_else(|_| crate::panic_with_message("validated state"))
    }

    #[must_use]
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

    pub fn into_inner(self) -> &'a mut T {
        self.target
    }

    pub fn contract(&mut self) -> &mut T {
        self.target
    }
}

impl<T, S: GuardSpec<T>> Deref for Guard<'_, T, S> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        self.target
    }
}

impl<T, S: GuardSpec<T>> DerefMut for Guard<'_, T, S> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.target
    }
}

#[must_use = "drops immediately if not bound"]
pub struct OnDrop<F: FnOnce()> {
    f: Option<F>,
}

impl<F: FnOnce()> OnDrop<F> {
    pub fn new(f: F) -> Self {
        Self { f: Some(f) }
    }

    pub fn disarm(mut self) {
        self.f = None;
    }
}

impl<F: FnOnce()> Drop for OnDrop<F> {
    fn drop(&mut self) {
        if let Some(f) = self.f.take() {
            f();
        }
    }
}

#[inline]
pub fn defer<F: FnOnce()>(f: F) -> OnDrop<F> {
    OnDrop::new(f)
}
