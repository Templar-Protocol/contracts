use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::macros::{read_method_spec, write_method_spec};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GetOwnerParams {
    pub oracle_id: near_account_id::AccountId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GetOwnerResult {
    pub owner: Option<near_account_id::AccountId>,
}

read_method_spec!(
    /// Get the current proxy oracle owner.
    "proxyOracleOwner.getOwner": GetOwner(GetOwnerParams) -> GetOwnerResult
);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GetProposedOwnerParams {
    pub oracle_id: near_account_id::AccountId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GetProposedOwnerResult {
    pub proposed_owner: Option<near_account_id::AccountId>,
}

read_method_spec!(
    /// Get the proposed proxy oracle owner.
    "proxyOracleOwner.getProposedOwner": GetProposedOwner(GetProposedOwnerParams) -> GetProposedOwnerResult
);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ProposeOwnerBody {
    pub oracle_id: near_account_id::AccountId,
    pub account_id: Option<near_account_id::AccountId>,
}
write_method_spec!(
    /// Propose a new proxy oracle owner.
    "proxyOracleOwner.proposeOwner": ProposeOwner(ProposeOwnerBody)
);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AcceptOwnerBody {
    pub oracle_id: near_account_id::AccountId,
}
write_method_spec!(
    /// Accept proxy oracle ownership.
    "proxyOracleOwner.acceptOwner": AcceptOwner(AcceptOwnerBody)
);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RenounceOwnerBody {
    pub oracle_id: near_account_id::AccountId,
}
write_method_spec!(
    /// Renounce proxy oracle ownership.
    "proxyOracleOwner.renounceOwner": RenounceOwner(RenounceOwnerBody)
);
