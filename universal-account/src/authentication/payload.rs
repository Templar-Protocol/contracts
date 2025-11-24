use alloy::sol;
use near_sdk::{near, AccountId};

use crate::ExecutionParameters;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[near(serializers = [json])]
#[serde(deny_unknown_fields)]
pub struct Payload<T> {
    pub parameters: ExecutionParameters,
    pub account_id: AccountId,
    pub payload: T,
}

sol! {
    #[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
    struct SolBytes {
        bytes inner;
    }
}
