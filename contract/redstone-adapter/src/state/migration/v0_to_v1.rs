use near_sdk::near;
use templar_common::versioned_state::StateTransformer;

use crate::state::{legacy::v0, State};

#[derive(Clone, Debug)]
#[near(serializers = [json])]
pub struct V0ToV1;

impl StateTransformer for V0ToV1 {
    type Input = v0::State;
    type Output = State;
    type Error = ();

    fn transform(&self, input: Self::Input) -> Result<Self::Output, Self::Error> {
        // Move the existing map handle and policy without touching historical entries or RBAC.
        Ok(State {
            adapter: input.adapter,
        })
    }
}
