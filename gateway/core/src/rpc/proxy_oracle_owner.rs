use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{
    macros::{public_read_method_spec, write_method_spec},
    rpc::common::WriteOperationResult,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GetOwnerParams {
    pub oracle_id: near_account_id::AccountId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GetOwnerResult {
    pub owner: Option<near_account_id::AccountId>,
}

public_read_method_spec!(
    GetOwner,
    "proxyOracleOwner.getOwner",
    GetOwnerParams,
    GetOwnerResult
);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GetProposedOwnerParams {
    pub oracle_id: near_account_id::AccountId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GetProposedOwnerResult {
    pub proposed_owner: Option<near_account_id::AccountId>,
}

public_read_method_spec!(
    GetProposedOwner,
    "proxyOracleOwner.getProposedOwner",
    GetProposedOwnerParams,
    GetProposedOwnerResult
);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ProposeOwnerBody {
    pub oracle_id: near_account_id::AccountId,
    pub account_id: Option<near_account_id::AccountId>,
}
pub type ProposeOwnerResult = WriteOperationResult;
write_method_spec!(
    ProposeOwner,
    "proxyOracleOwner.proposeOwner",
    ProposeOwnerBody,
    ProposeOwnerResult
);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AcceptOwnerBody {
    pub oracle_id: near_account_id::AccountId,
}
pub type AcceptOwnerResult = WriteOperationResult;
write_method_spec!(
    AcceptOwner,
    "proxyOracleOwner.acceptOwner",
    AcceptOwnerBody,
    AcceptOwnerResult
);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RenounceOwnerBody {
    pub oracle_id: near_account_id::AccountId,
}
pub type RenounceOwnerResult = WriteOperationResult;
write_method_spec!(
    RenounceOwner,
    "proxyOracleOwner.renounceOwner",
    RenounceOwnerBody,
    RenounceOwnerResult
);
