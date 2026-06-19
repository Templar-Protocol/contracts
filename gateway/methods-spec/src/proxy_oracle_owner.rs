use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use templar_gateway_macros::{read_method_spec, write_method_spec};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GetOwner {
    pub oracle_id: near_account_id::AccountId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GetOwnerResult {
    pub owner: Option<near_account_id::AccountId>,
}

read_method_spec!(
    /// Get the current proxy oracle owner.
    "proxyOracleOwner.getOwner": GetOwner -> GetOwnerResult
);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GetProposedOwner {
    pub oracle_id: near_account_id::AccountId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GetProposedOwnerResult {
    pub proposed_owner: Option<near_account_id::AccountId>,
}

read_method_spec!(
    /// Get the proposed proxy oracle owner.
    "proxyOracleOwner.getProposedOwner": GetProposedOwner -> GetProposedOwnerResult
);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ProposeOwner {
    pub oracle_id: near_account_id::AccountId,
    pub account_id: Option<near_account_id::AccountId>,
}
write_method_spec!(
    /// Propose a new proxy oracle owner.
    "proxyOracleOwner.proposeOwner": ProposeOwner
);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AcceptOwner {
    pub oracle_id: near_account_id::AccountId,
}
write_method_spec!(
    /// Accept proxy oracle ownership.
    "proxyOracleOwner.acceptOwner": AcceptOwner
);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RenounceOwner {
    pub oracle_id: near_account_id::AccountId,
}
write_method_spec!(
    /// Renounce proxy oracle ownership.
    "proxyOracleOwner.renounceOwner": RenounceOwner
);
