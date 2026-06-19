use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use templar_gateway_macros::MethodSpec;

/// Get the current proxy oracle owner.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(read = "proxyOracleOwner.getOwner", output = GetOwnerResult)]
pub struct GetOwner {
    pub oracle_id: near_account_id::AccountId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GetOwnerResult {
    pub owner: Option<near_account_id::AccountId>,
}

/// Get the proposed proxy oracle owner.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(read = "proxyOracleOwner.getProposedOwner", output = GetProposedOwnerResult)]
pub struct GetProposedOwner {
    pub oracle_id: near_account_id::AccountId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GetProposedOwnerResult {
    pub proposed_owner: Option<near_account_id::AccountId>,
}

/// Propose a new proxy oracle owner.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(write = "proxyOracleOwner.proposeOwner")]
pub struct ProposeOwner {
    pub oracle_id: near_account_id::AccountId,
    pub account_id: Option<near_account_id::AccountId>,
}

/// Accept proxy oracle ownership.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(write = "proxyOracleOwner.acceptOwner")]
pub struct AcceptOwner {
    pub oracle_id: near_account_id::AccountId,
}

/// Renounce proxy oracle ownership.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(write = "proxyOracleOwner.renounceOwner")]
pub struct RenounceOwner {
    pub oracle_id: near_account_id::AccountId,
}
