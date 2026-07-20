use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use templar_gateway_macros::MethodSpec;

/// Get the current contract owner.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(read = "owner.getOwner", output = GetOwnerResult)]
pub struct GetOwner {
    pub contract_id: near_account_id::AccountId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GetOwnerResult {
    pub owner: Option<near_account_id::AccountId>,
}

/// Get the proposed contract owner.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(read = "owner.getProposedOwner", output = GetProposedOwnerResult)]
pub struct GetProposedOwner {
    pub contract_id: near_account_id::AccountId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GetProposedOwnerResult {
    pub proposed_owner: Option<near_account_id::AccountId>,
}

/// Propose a new contract owner.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(write = "owner.proposeOwner")]
pub struct ProposeOwner {
    pub contract_id: near_account_id::AccountId,
    pub account_id: Option<near_account_id::AccountId>,
}

/// Accept contract ownership.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(write = "owner.acceptOwner")]
pub struct AcceptOwner {
    pub contract_id: near_account_id::AccountId,
}

/// Renounce contract ownership.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(write = "owner.renounceOwner")]
pub struct RenounceOwner {
    pub contract_id: near_account_id::AccountId,
}
